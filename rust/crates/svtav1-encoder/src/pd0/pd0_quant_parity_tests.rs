use super::*;
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
use svtav1_cref as cref;

#[test]
fn pd0_energy_matches_c_all_tiers() {
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        for (w, h) in [
            (4, 4),
            (8, 8),
            (16, 16),
            (32, 32),
            (64, 32),
            (32, 64),
            (64, 64),
        ] {
            for padding in [0, 7, 32] {
                for offset in [0, 1] {
                    let stride = w + padding;
                    // A large sentinel makes an accidental inclusion of
                    // row padding visible, including the right quadrant.
                    let mut coeff = vec![1_000_000; stride * h + offset];
                    for r in 0..h {
                        for c in 0..w {
                            coeff[offset + r * stride + c] =
                                ((r * 719 + c * 997) % 262_145) as i32 - 131_072;
                        }
                    }
                    let input = &coeff[offset..];
                    let expected = cref::pic_operators::full_distortion_kernel_cbf_zero32_bits(
                        input, stride, w, h,
                    );
                    assert_eq!(expected.0, expected.1);
                    assert_eq!(
                        energy(input, stride, w, h),
                        expected.0,
                        "{w}x{h} stride={stride} offset={offset}"
                    );
                }
            }
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}

#[test]
fn pd0_transform_outputs_match_both_quantizer_oracles() {
    use svtav1_types::transform::TxType;
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        for (w, h) in [
            (4, 4),
            (8, 4),
            (8, 8),
            (8, 16),
            (16, 8),
            (16, 16),
            (16, 32),
            (32, 16),
            (32, 32),
            (32, 64),
            (64, 32),
            (64, 64),
        ] {
            let (tx_size, c_tx) = pd0_tx_size(w, h);
            let scan = crate::entropy::scan_tables::scan(c_tx, 0);
            let log_scale = TX_SCALE_TAB[c_tx];
            let mut maximum = 0;
            // Actual 8-bit residuals: constant extrema, alternating extrema,
            // edges, impulses, and deterministic signed noise. This is a
            // producer regression grid, not a proof for every possible image.
            for pattern in 0..16 {
                let residual: Vec<i16> = (0..w * h)
                    .map(|i| match pattern {
                        0 => 255,
                        1 => -255,
                        2 => {
                            if (i / w + i % w) % 2 == 0 {
                                255
                            } else {
                                -255
                            }
                        }
                        3 => {
                            if i % w < w / 2 {
                                255
                            } else {
                                -255
                            }
                        }
                        4 => {
                            if i / w < h / 2 {
                                255
                            } else {
                                -255
                            }
                        }
                        5 => {
                            if i == 0 {
                                255
                            } else {
                                0
                            }
                        }
                        6 => {
                            if i + 1 == w * h {
                                -255
                            } else {
                                0
                            }
                        }
                        _ => {
                            ((i as u32)
                                .wrapping_mul(1664525)
                                .wrapping_add(pattern * 1013904223u32.wrapping_div(16))
                                % 511) as i16
                                - 255
                        }
                    })
                    .collect();
                let mut full = vec![0; w * h];
                assert!(svtav1_dsp::txfm_dispatch::fwd_txfm2d_dispatch(
                    &residual,
                    &mut full,
                    w,
                    tx_size,
                    TxType::DctDct
                ));
                let mut coeff = Vec::new();
                for y in 0..h.min(32) {
                    coeff.extend_from_slice(&full[y * w..y * w + w.min(32)]);
                }
                maximum = maximum.max(coeff.iter().map(|x| x.abs()).max().unwrap());
                for qindex in 0..=255 {
                    let e = build_quant_entry(qindex);
                    let narrow = |v: [i32; 2]| v.map(|x| i16::try_from(x).unwrap());
                    let row = cref::QuantRow::new(
                        narrow(e.zbin),
                        narrow(e.round),
                        narrow(e.quant),
                        narrow(e.quant_shift),
                        [0; 2],
                        [0; 2],
                        narrow(e.dequant),
                    );
                    let actual = quantize_b(&coeff, scan, &e, log_scale);
                    for dispatch in [false, true] {
                        let mut q = vec![0; coeff.len()];
                        let mut dq = vec![0; coeff.len()];
                        let eob = cref::quantize_b(
                            &coeff, &row, scan, log_scale, dispatch, &mut q, &mut dq,
                        );
                        assert_eq!(
                            actual,
                            (eob, q, dq),
                            "producer {w}x{h} pattern={pattern} qindex={qindex} dispatch={dispatch}"
                        );
                    }
                }
            }
            eprintln!(
                "PD0 producer {w}x{h}: 16 residual patterns, 256 qindices, max abs coefficient={maximum}"
            );
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}

#[cfg(target_arch = "aarch64")]
#[test]
fn c_neon_quantizer_wide_input_divergence_is_explicit() {
    let e = build_quant_entry(0);
    let narrow = |v: [i32; 2]| v.map(|x| i16::try_from(x).unwrap());
    let row = cref::QuantRow::new(
        narrow(e.zbin),
        narrow(e.round),
        narrow(e.quant),
        narrow(e.quant_shift),
        [0; 2],
        [0; 2],
        narrow(e.dequant),
    );
    let scan = crate::entropy::scan_tables::scan(0, 0);
    for value in [-131072, -32768, 32768, 131072] {
        let coeff = vec![value; 16];
        let scalar = quantize_b_scan_reference(&coeff, scan, &e, 0);
        let mut outputs = Vec::new();
        for dispatch in [false, true] {
            let mut q = vec![0; 16];
            let mut dq = vec![0; 16];
            let eob = cref::quantize_b(&coeff, &row, scan, 0, dispatch, &mut q, &mut dq);
            outputs.push((eob, q, dq));
        }
        assert_eq!(outputs[0], scalar, "C scalar value={value}");
        assert_ne!(outputs[0], outputs[1], "C oracle divergence value={value}");
        assert_eq!(
            outputs[1],
            (0, vec![0; 16], vec![0; 16]),
            "NEON narrowing value={value}"
        );
    }
}

#[test]
fn pd0_quantize_b_matches_c_all_tiers() {
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        for qindex in 0..=255u8 {
            let e = build_quant_entry(qindex);
            let narrow = |v: [i32; 2]| v.map(|x| i16::try_from(x).unwrap());
            let row = cref::QuantRow::new(
                narrow(e.zbin),
                narrow(e.round),
                narrow(e.quant),
                narrow(e.quant_shift),
                [0; 2],
                [0; 2],
                narrow(e.dequant),
            );
            // PD0's square and boundary-rectangle DCT scans, including
            // packed 64-dimension transforms at all three log scales.
            for c_tx in [0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12] {
                let scan = crate::entropy::scan_tables::scan(c_tx, 0);
                let log_scale = TX_SCALE_TAB[c_tx];
                for pattern in 0..4 {
                    let mut coeff = vec![0i32; scan.len()];
                    for (i, &rc) in scan.iter().enumerate() {
                        let rc = usize::from(rc);
                        let zbin =
                            (e.zbin[usize::from(rc != 0)] + ((1 << log_scale) >> 1)) >> log_scale;
                        coeff[rc] = match pattern {
                            0 => 0,
                            // Threshold equality, either sign, and a
                            // dead-zone suffix test prescan equivalence.
                            1 if i < scan.len() / 3 => {
                                [zbin - 1, zbin, zbin + 1, 1 - zbin, -zbin, -zbin - 1][i % 6]
                            }
                            1 => 0,
                            // Clamp boundaries and a broad, signed
                            // coefficient range, not a smooth spectrum.
                            2 => [
                                -131_072, -32_768, -32_767, -1023, 1023, 32_767, 32_768, 131_072,
                            ][i % 8],
                            // Only the final scan coefficient survives.
                            _ if i + 1 == scan.len() => 32_767,
                            _ => 0,
                        };
                    }
                    let actual = quantize_b(&coeff, scan, &e, log_scale);
                    assert_eq!(
                        actual,
                        quantize_b_scan_reference(&coeff, scan, &e, log_scale)
                    );
                    for dispatch in [false, true] {
                        // Preserve the broad clamp-boundary test against C
                        // scalar. C NEON narrows i32 to i16 before abs;
                        // its symmetric comparison domain excludes -32768.
                        // Producer-derived inputs are checked separately
                        // against BOTH oracles without any clamping.
                        let oracle_coeff: Vec<_> = coeff
                            .iter()
                            .map(|&v| if dispatch { v.clamp(-32767, 32767) } else { v })
                            .collect();
                        let actual = quantize_b(&oracle_coeff, scan, &e, log_scale);
                        assert_eq!(
                            actual,
                            quantize_b_scan_reference(&oracle_coeff, scan, &e, log_scale)
                        );
                        let mut q = vec![0x5555; scan.len()];
                        let mut dq = vec![0x5555; scan.len()];
                        let eob = cref::quantize_b(
                            &oracle_coeff,
                            &row,
                            scan,
                            log_scale,
                            dispatch,
                            &mut q,
                            &mut dq,
                        );
                        assert_eq!(
                            actual,
                            (eob, q, dq),
                            "qindex={qindex} tx={c_tx} pattern={pattern} C dispatch={dispatch}"
                        );
                    }
                }
            }
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}
