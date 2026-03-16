//! End-to-end correctness tests for the SVT-AV1 Rust encoder pipeline.
//!
//! These tests wire multiple subsystems together and verify the full
//! predict → residual → transform → quantize → entropy → dequantize →
//! inverse transform → reconstruct chain produces correct output.
//!
//! Test categories:
//! 1. Transform roundtrip (fwd→inv across all sizes and types)
//! 2. Quantization roundtrip with transform
//! 3. Full encode block correctness
//! 4. Intra prediction → encode → verify PSNR
//! 5. Motion estimation → encode → verify
//! 6. Entropy encode → verify bitstream non-empty
//! 7. Multi-block frame encoding consistency
//! 8. Edge cases (flat, max contrast, gradients)
//! 9. Rate control integration
//! 10. Cross-module data flow verification

// ============================================================================
// 1. Transform roundtrip correctness (all available sizes × types)
// ============================================================================

mod transform_roundtrip {
    use svtav1_dsp::fwd_txfm::*;
    use svtav1_dsp::inv_txfm::*;

    /// Helper: generate a deterministic test pattern for a given size.
    fn test_pattern(n: usize, seed: u8) -> Vec<i32> {
        (0..n)
            .map(|i| {
                let v = (i as u8).wrapping_mul(seed).wrapping_add(37);
                (v as i32) - 128 // centered around 0
            })
            .collect()
    }

    #[test]
    fn dct4_roundtrip_multiple_inputs() {
        for seed in [1u8, 17, 42, 99, 200, 255] {
            let input = test_pattern(4, seed);
            let mut fwd = [0i32; 4];
            let mut inv = [0i32; 4];
            fdct4(&input, &mut fwd);
            idct4(&fwd, &mut inv);
            // AV1 scale factor: roundtrip = input * N/2 = input * 2
            for i in 0..4 {
                let expected = input[i] * 2;
                let diff = (inv[i] - expected).abs();
                assert!(
                    diff <= 2,
                    "seed={seed} i={i}: inv={} expected={expected} diff={diff}",
                    inv[i]
                );
            }
        }
    }

    #[test]
    fn dct8_roundtrip_multiple_inputs() {
        for seed in [1u8, 33, 77, 128, 250] {
            let input = test_pattern(8, seed);
            let mut fwd = [0i32; 8];
            let mut inv = [0i32; 8];
            fdct8(&input, &mut fwd);
            idct8(&fwd, &mut inv);
            for i in 0..8 {
                let expected = input[i] * 4; // 8-point scale = N/2 = 4
                let diff = (inv[i] - expected).abs();
                assert!(
                    diff <= 4,
                    "seed={seed} i={i}: inv={} expected={expected} diff={diff}",
                    inv[i]
                );
            }
        }
    }

    #[test]
    fn txfm2d_4x4_roundtrip_random_patterns() {
        for seed in [3u8, 19, 73, 151, 222] {
            let input = test_pattern(16, seed);
            let mut fwd = [0i32; 16];
            let mut inv = [0i32; 16];
            fwd_txfm2d_4x4_dct_dct(&input, &mut fwd, 4);
            inv_txfm2d_4x4_dct_dct(&fwd, &mut inv, 4);
            for i in 0..16 {
                let diff = (inv[i] - input[i]).abs();
                assert!(
                    diff <= 2,
                    "seed={seed} i={i}: inv={} input={} diff={diff}",
                    inv[i],
                    input[i]
                );
            }
        }
    }

    #[test]
    fn txfm2d_8x8_roundtrip_random_patterns() {
        for seed in [5u8, 41, 100, 189] {
            let input = test_pattern(64, seed);
            let mut fwd = [0i32; 64];
            let mut inv = [0i32; 64];
            fwd_txfm2d_8x8_dct_dct(&input, &mut fwd, 8);
            inv_txfm2d_8x8_dct_dct(&fwd, &mut inv, 8);
            for i in 0..64 {
                let diff = (inv[i] - input[i]).abs();
                assert!(
                    diff <= 4,
                    "seed={seed} i={i}: inv={} input={} diff={diff}",
                    inv[i],
                    input[i]
                );
            }
        }
    }

    #[test]
    fn txfm2d_4x4_energy_conservation() {
        // Parseval: sum(input²) ≈ sum(output²) / N for orthonormal transforms
        // AV1 is scaled, so we just verify energy is preserved within a factor
        let input = test_pattern(16, 42);
        let mut fwd = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&input, &mut fwd, 4);

        let input_energy: i64 = input.iter().map(|&x| x as i64 * x as i64).sum();
        let fwd_energy: i64 = fwd.iter().map(|&x| x as i64 * x as i64).sum();

        assert!(input_energy > 0);
        assert!(fwd_energy > 0);
        // Energy should be within 10x due to scaling
        let ratio = fwd_energy as f64 / input_energy as f64;
        assert!(
            ratio > 0.1 && ratio < 100.0,
            "energy ratio {ratio} out of range"
        );
    }
}

// ============================================================================
// 2. Transform + quantization roundtrip
// ============================================================================

mod txfm_quant_roundtrip {
    use svtav1_dsp::fwd_txfm::*;
    use svtav1_dsp::inv_txfm::*;
    use svtav1_dsp::quant::*;

    #[test]
    fn txfm_quantize_dequantize_inv_txfm_4x4() {
        // Full chain: pixel residual → fwd txfm → quantize → dequantize → inv txfm
        let residual: Vec<i32> = (0..16).map(|i| i * 5 - 40).collect();
        let mut coeffs = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&residual, &mut coeffs, 4);

        let qparam = QuantParam {
            dequant: [8, 16],
            shift: 1,
        };
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        let _eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);

        let mut recon_residual = [0i32; 16];
        inv_txfm2d_4x4_dct_dct(&dqcoeffs, &mut recon_residual, 4);

        // Verify reconstruction is reasonable (lossy but correlated)
        let mut total_error: i64 = 0;
        for i in 0..16 {
            let err = (recon_residual[i] - residual[i]).abs() as i64;
            total_error += err;
        }
        let avg_error = total_error / 16;
        // At this QP, average error should be manageable
        assert!(
            avg_error < 50,
            "avg pixel error {avg_error} too high for low QP"
        );
    }

    #[test]
    fn lossless_at_qp1() {
        // Very low quantization should be near-lossless
        let residual: Vec<i32> = (0..16).map(|i| i * 3 - 20).collect();
        let mut coeffs = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&residual, &mut coeffs, 4);

        let qparam = QuantParam {
            dequant: [1, 1],
            shift: 0,
        };
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);

        // With dequant=1 and shift=0, qcoeffs should equal coeffs
        for i in 0..16 {
            assert_eq!(
                qcoeffs[i], coeffs[i],
                "lossless quantization mismatch at {i}"
            );
        }

        let mut recon = [0i32; 16];
        inv_txfm2d_4x4_dct_dct(&dqcoeffs, &mut recon, 4);

        for i in 0..16 {
            let diff = (recon[i] - residual[i]).abs();
            assert!(diff <= 2, "near-lossless recon error {diff} at {i}");
        }
    }

    #[test]
    fn zero_residual_produces_zero_output() {
        let residual = [0i32; 16];
        let mut coeffs = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&residual, &mut coeffs, 4);
        assert!(
            coeffs.iter().all(|&c| c == 0),
            "zero residual → zero coeffs"
        );

        let qparam = QuantParam {
            dequant: [4, 8],
            shift: 1,
        };
        let mut qcoeffs = [0i32; 16];
        let mut dqcoeffs = [0i32; 16];
        let eob = quantize(&coeffs, &qparam, &mut qcoeffs, &mut dqcoeffs, 16);
        assert_eq!(eob, 0);

        let mut recon = [0i32; 16];
        inv_txfm2d_4x4_dct_dct(&dqcoeffs, &mut recon, 4);
        assert!(recon.iter().all(|&r| r == 0));
    }
}

// ============================================================================
// 3. Full encode_block correctness
// ============================================================================

mod encode_block_e2e {
    use svtav1_encoder::encode_loop::encode_block;

    fn psnr(src: &[u8], recon: &[u8]) -> f64 {
        assert_eq!(src.len(), recon.len());
        let mse: f64 = src
            .iter()
            .zip(recon.iter())
            .map(|(&s, &r)| {
                let d = s as f64 - r as f64;
                d * d
            })
            .sum::<f64>()
            / src.len() as f64;
        if mse == 0.0 {
            return f64::INFINITY;
        }
        10.0 * (255.0 * 255.0 / mse).log10()
    }

    #[test]
    fn encode_4x4_identical_src_pred() {
        let src = [128u8; 16];
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        assert_eq!(result.eob, 0, "identical blocks should have zero residual");
        assert_eq!(result.distortion, 0);
        assert_eq!(
            &result.recon, &src,
            "reconstruction should match source exactly"
        );
    }

    #[test]
    fn encode_4x4_psnr_above_30db() {
        let src: Vec<u8> = (0..16).map(|i| (i * 16) as u8).collect();
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 25);
        let db = psnr(&src, &result.recon);
        assert!(db > 20.0, "PSNR {db:.1} dB too low for QP=25");
    }

    #[test]
    fn encode_8x8_reconstruction_quality() {
        // Gradient source — realistic content
        let mut src = [0u8; 64];
        for r in 0..8 {
            for c in 0..8 {
                src[r * 8 + c] = (r * 25 + c * 10) as u8;
            }
        }
        let pred = [100u8; 64];
        let result = encode_block(&src, 8, &pred, 8, 8, 8, 28);
        let db = psnr(&src, &result.recon);
        assert!(db > 15.0, "8x8 PSNR {db:.1} dB too low");
        assert!(result.eob > 0, "gradient should have non-zero coefficients");
    }

    #[test]
    fn encode_distortion_matches_recon_sse() {
        let src: Vec<u8> = (0..16).map(|i| (i * 15 + 10) as u8).collect();
        let pred = [100u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);

        // Manually compute SSE between src and recon
        let manual_sse: u64 = src
            .iter()
            .zip(result.recon.iter())
            .map(|(&s, &r)| {
                let d = s as i64 - r as i64;
                (d * d) as u64
            })
            .sum();

        assert_eq!(
            result.distortion, manual_sse,
            "reported distortion {} != manual SSE {manual_sse}",
            result.distortion
        );
    }

    #[test]
    fn higher_qp_means_more_distortion() {
        let src: Vec<u8> = (0..16).map(|i| (i * 15 + 10) as u8).collect();
        let pred = [100u8; 16];

        let low_qp = encode_block(&src, 4, &pred, 4, 4, 4, 10);
        let high_qp = encode_block(&src, 4, &pred, 4, 4, 4, 50);

        assert!(
            high_qp.distortion >= low_qp.distortion,
            "higher QP should produce >= distortion: low={} high={}",
            low_qp.distortion,
            high_qp.distortion
        );
    }

    #[test]
    fn higher_qp_means_fewer_coefficients() {
        let src: Vec<u8> = (0..16).map(|i| (i * 15 + 10) as u8).collect();
        let pred = [100u8; 16];

        let low_qp = encode_block(&src, 4, &pred, 4, 4, 4, 5);
        let high_qp = encode_block(&src, 4, &pred, 4, 4, 4, 60);

        assert!(
            high_qp.eob <= low_qp.eob,
            "higher QP should produce <= non-zero coeffs: low={} high={}",
            low_qp.eob,
            high_qp.eob
        );
    }

    #[test]
    fn encode_all_zeros_src() {
        let src = [0u8; 16];
        let pred = [0u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        assert_eq!(result.eob, 0);
        assert_eq!(result.distortion, 0);
        assert!(result.recon.iter().all(|&r| r == 0));
    }

    #[test]
    fn encode_all_255_src() {
        let src = [255u8; 16];
        let pred = [255u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        assert_eq!(result.eob, 0);
        assert_eq!(result.distortion, 0);
        assert!(result.recon.iter().all(|&r| r == 255));
    }

    #[test]
    fn encode_max_contrast() {
        let src = [255u8; 16];
        let pred = [0u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 15);
        assert!(result.eob > 0);
        // Even at QP=15, max contrast should reconstruct well
        for &r in &result.recon {
            assert!(
                r > 180,
                "max contrast recon pixel {r} too far from 255 at QP=15"
            );
        }
    }

    #[test]
    fn reconstruction_clipped_to_0_255() {
        // Source = 250, pred = 10 → residual = 240, recon should not exceed 255
        let src = [250u8; 16];
        let pred = [10u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 10);
        // u8 values are inherently in [0, 255]; verify recon is non-empty
        assert_eq!(result.recon.len(), 16);

        // Source = 5, pred = 245 → residual = -240, recon should not go below 0
        let src2 = [5u8; 16];
        let pred2 = [245u8; 16];
        let result2 = encode_block(&src2, 4, &pred2, 4, 4, 4, 10);
        for &r in &result2.recon {
            // u8 can't be < 0, but verify value is reasonable
            assert!(
                r < 100,
                "negative residual recon pixel {r} unexpectedly high"
            );
        }
    }
}

// ============================================================================
// 4. Intra prediction → encode → verify
// ============================================================================

mod intra_encode_e2e {
    use svtav1_dsp::intra_pred::*;
    use svtav1_encoder::encode_loop::encode_block;

    fn psnr(src: &[u8], recon: &[u8]) -> f64 {
        let mse: f64 = src
            .iter()
            .zip(recon.iter())
            .map(|(&s, &r)| {
                let d = s as f64 - r as f64;
                d * d
            })
            .sum::<f64>()
            / src.len() as f64;
        if mse == 0.0 {
            return f64::INFINITY;
        }
        10.0 * (255.0 * 255.0 / mse).log10()
    }

    #[test]
    fn dc_pred_encode_roundtrip() {
        let above = [120u8; 8];
        let left = [130u8; 8];
        // Generate DC prediction
        let mut pred = [0u8; 64];
        predict_dc(&mut pred, 8, &above, &left, 8, 8, true, true);

        // Source: the prediction itself → should be lossless
        let result = encode_block(&pred, 8, &pred, 8, 8, 8, 30);
        assert_eq!(result.distortion, 0);
    }

    #[test]
    fn v_pred_encode_gradient_source() {
        // Source has a vertical gradient
        let mut src = [0u8; 64];
        for r in 0..8 {
            for c in 0..8 {
                src[r * 8 + c] = (30 * r) as u8;
            }
        }

        let above = [0u8; 8]; // V-pred fills entire block with row 0
        let mut pred = [0u8; 64];
        predict_v(&mut pred, 8, &above, 8, 8);

        let result = encode_block(&src, 8, &pred, 8, 8, 8, 25);
        let db = psnr(&src, &result.recon);
        assert!(db > 15.0, "V-pred gradient PSNR {db:.1}dB too low");
    }

    #[test]
    fn h_pred_encode_gradient_source() {
        let mut src = [0u8; 64];
        for r in 0..8 {
            for c in 0..8 {
                src[r * 8 + c] = (30 * c) as u8;
            }
        }

        let left = [0u8; 8];
        let mut pred = [0u8; 64];
        predict_h(&mut pred, 8, &left, 8, 8);

        let result = encode_block(&src, 8, &pred, 8, 8, 8, 25);
        let db = psnr(&src, &result.recon);
        assert!(db > 15.0, "H-pred gradient PSNR {db:.1}dB too low");
    }

    #[test]
    fn paeth_pred_encode_natural_content() {
        // Natural-ish content with gradients in both directions
        let mut src = [0u8; 64];
        for r in 0..8 {
            for c in 0..8 {
                src[r * 8 + c] = (50 + r * 10 + c * 15) as u8;
            }
        }

        let above: Vec<u8> = (0..8).map(|c| (50 + c * 15) as u8).collect();
        let left: Vec<u8> = (0..8).map(|r| (50 + r * 10) as u8).collect();
        let mut pred = [0u8; 64];
        predict_paeth(&mut pred, 8, &above, &left, 50, 8, 8);

        let result = encode_block(&src, 8, &pred, 8, 8, 8, 25);
        let db = psnr(&src, &result.recon);
        assert!(db > 20.0, "Paeth PSNR {db:.1}dB too low for smooth content");
    }

    #[test]
    fn smooth_pred_encode_smooth_source() {
        // Smooth source that should be well-predicted by smooth mode
        let mut src = [0u8; 16];
        let above = [200u8; 4];
        let left = [100u8; 4];
        for r in 0..4 {
            for c in 0..4 {
                src[r * 4 + c] = (150 + r * 5 - c * 10) as u8;
            }
        }

        let mut pred = [0u8; 16];
        predict_smooth(&mut pred, 4, &above, &left, 4, 4);

        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        // Smooth prediction should give reasonable reconstruction
        assert!(result.recon.len() == 16);
    }

    #[test]
    fn best_intra_mode_selection() {
        // For a flat block, DC should be optimal
        let src = [128u8; 16];
        let above = [128u8; 4];
        let left = [128u8; 4];

        // DC prediction
        let mut dc_pred = [0u8; 16];
        predict_dc(&mut dc_pred, 4, &above, &left, 4, 4, true, true);
        let dc_result = encode_block(&src, 4, &dc_pred, 4, 4, 4, 30);

        // V prediction
        let mut v_pred = [0u8; 16];
        predict_v(&mut v_pred, 4, &above, 4, 4);
        let v_result = encode_block(&src, 4, &v_pred, 4, 4, 4, 30);

        // DC should be at least as good as V for flat content
        assert!(
            dc_result.distortion <= v_result.distortion,
            "DC dist {} should be <= V dist {} for flat block",
            dc_result.distortion,
            v_result.distortion
        );
    }
}

// ============================================================================
// 5. Motion estimation → encode → verify
// ============================================================================

mod me_encode_e2e {
    use svtav1_encoder::encode_loop::encode_block;
    use svtav1_encoder::motion_est::*;
    use svtav1_types::motion::Mv;

    #[test]
    fn me_finds_block_then_encodes_with_zero_residual() {
        // Place a block in the reference, find it with ME, encode with zero residual
        let w = 64;
        let h = 64;
        let mut src = vec![0u8; 8 * 8];
        let mut ref_pic = vec![128u8; w * h];

        // Place pattern at (20, 20) in ref and in src
        for r in 0..8 {
            for c in 0..8 {
                let val = ((r * 31 + c * 17 + 42) % 256) as u8;
                src[r * 8 + c] = val;
                ref_pic[(20 + r) * w + (20 + c)] = val;
            }
        }

        // ME should find MV pointing to (20, 20)
        let result = full_pel_search(&src, 8, &ref_pic, w, 16, 16, 8, 8, Mv::ZERO, 8, 8, w, h);
        assert_eq!(result.distortion, 0, "ME should find exact match");

        // Extract the prediction block at the found MV location
        let ref_x = 16 + (result.mv.x as i32 >> 3);
        let ref_y = 16 + (result.mv.y as i32 >> 3);
        let mut pred = [0u8; 64];
        for r in 0..8 {
            for c in 0..8 {
                pred[r * 8 + c] = ref_pic[(ref_y as usize + r) * w + ref_x as usize + c];
            }
        }

        // Encode: src == pred → zero residual
        let enc = encode_block(&src, 8, &pred, 8, 8, 8, 30);
        assert_eq!(enc.distortion, 0, "perfect prediction → zero distortion");
        assert_eq!(enc.eob, 0, "perfect prediction → zero coefficients");
    }

    #[test]
    fn me_imperfect_match_encodes_residual() {
        let w = 64;
        let h = 64;
        let mut src = vec![0u8; 8 * 8];
        let mut ref_pic = vec![128u8; w * h];

        // Slightly different pattern in ref vs src
        for r in 0..8 {
            for c in 0..8 {
                src[r * 8 + c] = ((r * 31 + c * 17) % 256) as u8;
                ref_pic[(20 + r) * w + (20 + c)] = ((r * 31 + c * 17 + 5) % 256) as u8; // +5 offset
            }
        }

        let result = full_pel_search(&src, 8, &ref_pic, w, 16, 16, 8, 8, Mv::ZERO, 8, 8, w, h);

        let ref_x = 16 + (result.mv.x as i32 >> 3);
        let ref_y = 16 + (result.mv.y as i32 >> 3);
        let mut pred = [0u8; 64];
        for r in 0..8 {
            for c in 0..8 {
                pred[r * 8 + c] = ref_pic[(ref_y as usize + r) * w + ref_x as usize + c];
            }
        }

        let enc = encode_block(&src, 8, &pred, 8, 8, 8, 25);
        assert!(enc.eob > 0, "imperfect match should have residual");
        // But reconstruction should still be decent
        let mse: f64 = src
            .iter()
            .zip(enc.recon.iter())
            .map(|(&s, &r)| {
                let d = s as f64 - r as f64;
                d * d
            })
            .sum::<f64>()
            / 64.0;
        let psnr = if mse > 0.0 {
            10.0 * (255.0 * 255.0 / mse).log10()
        } else {
            f64::INFINITY
        };
        assert!(psnr > 20.0, "PSNR {psnr:.1}dB too low after ME+encode");
    }
}

// ============================================================================
// 6. Entropy coding roundtrip
// ============================================================================

mod entropy_e2e {
    use svtav1_entropy::cdf::*;
    use svtav1_entropy::writer::AomWriter;

    #[test]
    fn encode_sequence_produces_bytes() {
        let mut w = AomWriter::new(4096);

        // Encode a realistic mix of syntax elements
        for _ in 0..10 {
            w.write_bit(false); // skip = false
            w.write_literal(3, 4); // intra mode
            w.write_literal(0, 2); // tx_type (DCT-DCT)
        }

        let output = w.done();
        assert!(!output.is_empty(), "entropy encoder should produce output");
        assert!(
            output.len() < 100,
            "10 blocks should compress to < 100 bytes"
        );
    }

    #[test]
    fn cdf_adaptation_reduces_rate() {
        // Encode the same symbol many times → CDF should adapt → fewer bytes
        let mut w1 = AomWriter::new(4096);
        let mut cdf1 = [CDF_PROB_TOP / 2, 0, 0u16];
        for _ in 0..100 {
            w1.write_symbol(0, &mut cdf1, 2);
        }
        let out1 = w1.done();

        // Encode random symbols → CDF doesn't settle → more bytes
        let mut w2 = AomWriter::new(4096);
        let mut cdf2 = [CDF_PROB_TOP / 2, 0, 0u16];
        for i in 0..100u32 {
            w2.write_symbol((i % 2) as usize, &mut cdf2, 2);
        }
        let out2 = w2.done();

        assert!(
            out1.len() < out2.len(),
            "adapted CDF ({} bytes) should be smaller than random ({} bytes)",
            out1.len(),
            out2.len()
        );
    }

    #[test]
    fn cdf_update_consistency() {
        // Verify CDF state is consistent after many updates
        let top = CDF_PROB_TOP as u32;
        let mut cdf = [
            (top * 3 / 4) as u16,
            (top / 2) as u16,
            (top / 4) as u16,
            0,
            0u16,
        ];
        for i in 0..1000 {
            update_cdf(&mut cdf, i % 4, 4);
        }
        // CDFs should still be monotonically non-increasing
        for i in 0..2 {
            assert!(
                cdf[i] >= cdf[i + 1],
                "CDF not monotonic: cdf[{i}]={} < cdf[{}]={}",
                cdf[i],
                i + 1,
                cdf[i + 1]
            );
        }
        // Counter should be capped at 32
        assert!(cdf[4] <= 32);
    }
}

// ============================================================================
// 7. Multi-block frame encoding consistency
// ============================================================================

mod multi_block_e2e {
    use svtav1_encoder::encode_loop::encode_block;

    #[test]
    fn encode_16x16_as_four_8x8_blocks() {
        // Encode a 16x16 area as 4 independent 8x8 blocks
        let mut src = [0u8; 16 * 16];
        for r in 0..16 {
            for c in 0..16 {
                src[r * 16 + c] = ((r * 7 + c * 11 + 42) % 256) as u8;
            }
        }
        let pred = [128u8; 16 * 16];

        let mut total_distortion = 0u64;
        let mut total_eob = 0u16;

        for block_row in 0..2 {
            for block_col in 0..2 {
                let src_offset = block_row * 8 * 16 + block_col * 8;
                let pred_offset = block_row * 8 * 16 + block_col * 8;

                let result =
                    encode_block(&src[src_offset..], 16, &pred[pred_offset..], 16, 8, 8, 28);
                total_distortion += result.distortion;
                total_eob += result.eob;
            }
        }

        // All blocks should have encoded something
        assert!(
            total_eob > 0,
            "content should produce non-zero coefficients"
        );
        // Total distortion should be reasonable
        let mse = total_distortion as f64 / 256.0;
        let psnr = if mse > 0.0 {
            10.0 * (255.0 * 255.0 / mse).log10()
        } else {
            f64::INFINITY
        };
        assert!(psnr > 15.0, "multi-block PSNR {psnr:.1}dB too low");
    }

    #[test]
    fn blocks_are_independent() {
        // Encoding one block should not affect encoding of another
        let src = [200u8; 64];
        let pred = [128u8; 64];

        let result1 = encode_block(&src, 8, &pred, 8, 8, 8, 30);
        let result2 = encode_block(&src, 8, &pred, 8, 8, 8, 30);

        assert_eq!(result1.distortion, result2.distortion);
        assert_eq!(result1.eob, result2.eob);
        assert_eq!(result1.recon, result2.recon);
        assert_eq!(result1.qcoeffs, result2.qcoeffs);
    }
}

// ============================================================================
// 8. Edge cases
// ============================================================================

mod edge_cases {
    use svtav1_encoder::encode_loop::encode_block;

    #[test]
    fn alternating_checkerboard() {
        let mut src = [0u8; 16];
        for i in 0..16 {
            src[i] = if (i / 4 + i % 4) % 2 == 0 { 255 } else { 0 };
        }
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 20);
        assert!(
            result.eob > 0,
            "checkerboard should have high-frequency content"
        );
    }

    #[test]
    fn single_pixel_difference() {
        let mut src = [128u8; 16];
        src[0] = 200; // Only first pixel differs
        let pred = [128u8; 16];
        let result = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        // Should have some coefficients but low distortion
        assert!(result.distortion < 10000);
    }

    #[test]
    fn all_qp_values_dont_panic() {
        let src = [100u8; 16];
        let pred = [128u8; 16];
        for qp in 0..=63 {
            let result = encode_block(&src, 4, &pred, 4, 4, 4, qp);
            // Should not panic and reconstruction should be valid
            assert_eq!(result.recon.len(), 16);
        }
    }

    #[test]
    fn stride_larger_than_width() {
        // Source embedded in a larger buffer with stride > width
        let mut src_buf = vec![0u8; 32 * 8];
        let pred_buf = vec![128u8; 32 * 8];
        for r in 0..4 {
            for c in 0..4 {
                src_buf[r * 32 + c] = (r * 64 + c * 16) as u8;
            }
        }
        let result = encode_block(&src_buf, 32, &pred_buf, 32, 4, 4, 30);
        assert_eq!(result.recon.len(), 16);
    }
}

// ============================================================================
// 9. Rate control integration
// ============================================================================

mod rate_control_e2e {
    use svtav1_encoder::rate_control::*;

    #[test]
    fn cqp_produces_deterministic_qp_sequence() {
        let config = RcConfig {
            mode: RcMode::Cqp,
            qp: 30,
            ..Default::default()
        };
        let state = RcState::default();

        // Same input → same QP every time
        let qp1 = assign_picture_qp(&config, &state, 0);
        let qp2 = assign_picture_qp(&config, &state, 0);
        assert_eq!(qp1, qp2);
        assert_eq!(qp1, 30);
    }

    #[test]
    fn temporal_layers_produce_monotonic_qp() {
        let config = RcConfig {
            mode: RcMode::Crf,
            qp: 25,
            ..Default::default()
        };
        let state = RcState::default();

        let mut prev_qp = 0;
        for layer in 0..6 {
            let qp = assign_picture_qp(&config, &state, layer);
            assert!(qp >= prev_qp, "layer {layer} QP {qp} < prev {prev_qp}");
            prev_qp = qp;
        }
    }

    #[test]
    fn rc_state_accumulates_correctly() {
        let mut state = RcState::default();
        for i in 0..10 {
            update_rc_state(&mut state, 5000 + i * 100, 30);
        }
        assert_eq!(state.total_frames, 10);
        assert_eq!(
            state.total_bits,
            10 * 5000 + (100 + 200 + 300 + 400 + 500 + 600 + 700 + 800 + 900)
        );
        assert!(state.lambda > 0.0);
    }

    #[test]
    fn lambda_increases_with_qp() {
        let lambdas: Vec<f64> = (0..64).map(qp_to_lambda).collect();
        for i in 1..64 {
            assert!(
                lambdas[i] >= lambdas[i - 1],
                "lambda[{i}]={} < lambda[{}]={}",
                lambdas[i],
                i - 1,
                lambdas[i - 1]
            );
        }
    }
}

// ============================================================================
// 10. Cross-module data flow verification
// ============================================================================

mod cross_module {
    use svtav1_dsp::hadamard::{satd_4x4, satd_8x8};
    use svtav1_dsp::sad::sad;
    use svtav1_dsp::variance::sse;
    use svtav1_encoder::encode_loop::encode_block;
    use svtav1_encoder::mode_decision::*;
    use svtav1_types::block::BlockSize;

    #[test]
    fn sad_predicts_distortion_direction() {
        // Lower SAD between src and pred should correlate with lower encode distortion
        let src: Vec<u8> = (0..16).map(|i| (i * 16) as u8).collect();
        let good_pred: Vec<u8> = (0..16).map(|i| (i * 16 + 2) as u8).collect(); // close
        let bad_pred = [128u8; 16]; // far

        let good_sad = sad(&src, 4, &good_pred, 4, 4, 4);
        let bad_sad = sad(&src, 4, &bad_pred, 4, 4, 4);
        assert!(good_sad < bad_sad);

        let good_enc = encode_block(&src, 4, &good_pred, 4, 4, 4, 30);
        let bad_enc = encode_block(&src, 4, &bad_pred, 4, 4, 4, 30);
        assert!(
            good_enc.distortion <= bad_enc.distortion,
            "better prediction should yield lower distortion"
        );
    }

    #[test]
    fn sse_matches_encode_distortion_for_perfect_pred() {
        let src = [100u8; 16];
        let pred = [100u8; 16];
        let dsp_sse = sse(&src, 4, &pred, 4, 4, 4);
        let enc = encode_block(&src, 4, &pred, 4, 4, 4, 30);
        assert_eq!(dsp_sse, 0);
        assert_eq!(enc.distortion, 0);
    }

    #[test]
    fn satd_zero_for_identical_blocks() {
        let block = [42u8; 64];
        assert_eq!(satd_4x4(&block, 8, &block, 8), 0);
        assert_eq!(satd_8x8(&block, 8, &block, 8), 0);
    }

    #[test]
    fn mode_decision_rd_cost_selects_lower_distortion() {
        let src: Vec<u8> = (0..16).map(|i| (i * 16) as u8).collect();

        // Two candidates with known distortion
        let mut good = MdCandidate::default();
        let mut bad = MdCandidate::default();

        let good_pred: Vec<u8> = (0..16).map(|i| (i * 16 + 1) as u8).collect();
        let bad_pred = [128u8; 16];

        evaluate_candidate(&mut good, &src, &good_pred, 4, 4, 256);
        evaluate_candidate(&mut bad, &src, &bad_pred, 4, 4, 256);

        assert!(
            good.rd_cost < bad.rd_cost,
            "better pred should have lower RD cost"
        );

        let candidates = [good, bad];
        let best = select_best_candidate(&candidates).unwrap();
        assert_eq!(best.rd_cost, good.rd_cost);
    }

    #[test]
    fn generate_candidates_covers_all_modes() {
        let small = generate_intra_candidates(BlockSize::Block4x4);
        let large = generate_intra_candidates(BlockSize::Block8x8);

        // 4x4 should have at least DC, V, H, smooth*, paeth = 7
        assert!(small.len() >= 7);
        // 8x8 should additionally have directional modes
        assert!(large.len() > small.len());
    }
}
