use super::*;
use alloc::vec;
use alloc::vec::Vec;

/// The branch-free rounding in [`cfl_predict_lbd_core`] must equal the
/// branchy form over the WHOLE reachable domain: every `ac_q3` an `i16` can
/// hold, times every legal `alpha_q3` (C `cfl_idx_to_alpha`: magnitude
/// 1..=16 either sign, plus 0). 2,162,720 pairs, exhaustive — not sampled.
#[test]
fn cfl_branch_free_rounding_matches_the_branchy_form() {
    for alpha_mag in 0..=16i32 {
        for sign in [1i32, -1] {
            let alpha_q3 = alpha_mag * sign;
            for ac in i16::MIN..=i16::MAX {
                let q6 = alpha_q3 * ac as i32;
                let want = if q6 < 0 {
                    -((-q6 + 32) >> 6)
                } else {
                    (q6 + 32) >> 6
                };
                let s = q6 >> 31;
                let got = ((((q6 ^ s) - s) + 32) >> 6) ^ s;
                assert_eq!(got - s, want, "alpha={alpha_q3} ac={ac} q6={q6}");
            }
        }
    }
}

#[allow(dead_code)]
fn make_test_block(width: usize, height: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>, u8) {
    // above: increasing values
    let above: Vec<u8> = (0..width).map(|i| (100 + i) as u8).collect();
    // left: decreasing values
    let left: Vec<u8> = (0..height).map(|i| (200 - i * 5) as u8).collect();
    let dst = vec![0u8; width * height];
    let top_left = 150u8;
    (dst, above, left, top_left)
}

#[test]
fn dc_uniform_neighbors() {
    let above = [100u8; 4];
    let left = [100u8; 4];
    let mut dst = [0u8; 16];
    predict_dc(&mut dst, 4, &above, &left, 4, 4, true, true);
    assert!(dst.iter().all(|&v| v == 100));
}

/// The dispatched `predict_dc` (v3/neon/scalar via `incant!`) must equal
/// `predict_dc_core` on every size, flag combination, and stride — the
/// edge-sum is the only vectorized part, and a wrong sum flips the whole
/// block's dc value. Runs under every token permutation so the SIMD arm
/// and the scalar arm are BOTH pinned against the core.
#[test]
fn predict_dc_dispatch_matches_core_all_sizes_flags() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut seed = 0x54321u32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
        for &width in &[4usize, 8, 16, 32, 64] {
            for &height in &[4usize, 8, 16, 32, 64] {
                let above: Vec<u8> = (0..width).map(|_| next()).collect();
                let left: Vec<u8> = (0..height).map(|_| next()).collect();
                for &(ha, hl) in &[(true, true), (true, false), (false, true), (false, false)] {
                    for pad in [0usize, 5] {
                        let stride = width + pad;
                        let mut want = vec![0u8; stride * height];
                        let mut got = vec![0u8; stride * height];
                        predict_dc_core(&mut want, stride, &above, &left, width, height, ha, hl);
                        predict_dc(&mut got, stride, &above, &left, width, height, ha, hl);
                        assert_eq!(
                            want, got,
                            "predict_dc {width}x{height} stride {stride} flags ({ha},{hl}) \
                                 tier {perm}"
                        );
                    }
                }
            }
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2, "{report:?}");
}

/// The dispatched `predict_smooth` (v3/neon/scalar via `incant!`) must
/// equal `predict_smooth_core` on every AV1 block size — a SIMD kernel
/// that differs from its twin makes the bitstream tier-dependent.
/// Regression witness for the 4c-slot indexing fix (see git log): the
/// 4x4-only `smooth_corners` test never exercised the SIMD path.
#[test]
fn predict_smooth_dispatch_matches_core_all_sizes() {
    let mut seed = 0x12345u32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    for &width in &[4usize, 8, 16, 32, 64] {
        for &height in &[4usize, 8, 16, 32, 64] {
            let above: Vec<u8> = (0..width).map(|_| next()).collect();
            let left: Vec<u8> = (0..height).map(|_| next()).collect();
            let mut want = vec![0u8; width * height];
            let mut got = vec![0u8; width * height];
            predict_smooth_core(&mut want, width, &above, &left, width, height);
            predict_smooth(&mut got, width, &above, &left, width, height);
            if let Some(i) = want.iter().zip(&got).position(|(a, b)| a != b) {
                panic!(
                    "predict_smooth {width}x{height} diverges at idx {i} \
                         (row {}, col {}): want {} got {}",
                    i / width,
                    i % width,
                    want[i],
                    got[i]
                );
            }
        }
    }
}

#[test]
fn dr_z2_edged_dispatch_matches_core_all_sizes_flags_angles() {
    // Exercises dr_z2_edged's dispatch arms (the >= 16 staircase NEON,
    // the 4xH/8xH table-gather NEON, and the scalar tail) against the
    // scalar core over every upsample combination, all block sizes the
    // arms take, and (dx, dy) pairs spanning shallow to steep slopes —
    // including dy large enough to push base_x past the load-clamp
    // region (fully left-region rows).
    let mut seed = 0x2468u32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    let origin = EDGE_ORIGIN;
    for &bw in &[4usize, 8, 16, 32, 64] {
        for &bh in &[4usize, 8, 16, 32] {
            let above: Vec<u8> = (0..EDGE_BUF_LEN).map(|_| next()).collect();
            let left: Vec<u8> = (0..EDGE_BUF_LEN).map(|_| next()).collect();
            // Real angle-derived (dx, dy) pairs — the inverse slope
            // relationship is what keeps base_y >= -1 inside the left
            // region. (547, 3) + upsample_above drives base_x to -18 at
            // r = 0, exercising the clamped load on a fully-masked row.
            for &(dx, dy) in &[
                (27i32, 151i32),
                (151, 27),
                (64, 64),
                (7, 372),
                (372, 7),
                (3, 1023),
                (1023, 3),
                (547, 3),
            ] {
                for &(ua, ul) in &[(false, false), (true, false), (false, true), (true, true)] {
                    let mut want = vec![0u8; bw * bh];
                    let mut got = vec![0u8; bw * bh];
                    dr_z2_edged_core(&mut want, bw, bw, bh, &above, &left, origin, ua, ul, dx, dy);
                    dr_z2_edged(&mut got, bw, bw, bh, &above, &left, origin, ua, ul, dx, dy);
                    if want != got {
                        let i = want.iter().zip(&got).position(|(a, b)| a != b).unwrap();
                        panic!(
                            "dr_z2_edged {bw}x{bh} dx={dx} dy={dy} ua={ua} ul={ul} \
                                 diverges at ({}, {}): want {} got {}",
                            i / bw,
                            i % bw,
                            want[i],
                            got[i]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn dr_z1_edged_dispatch_matches_core_all_sizes_flags_angles() {
    // dr_z1_edged's arms — the 4xH/8xH row-vector NEON (with and
    // without upsample) and the >= 16 flat NEON — against the scalar
    // core over every size/flag/angle the arms take. The dx set is
    // the legal `DR_INTRA_DERIVATIVE` range, shallow to steep, so
    // `base` crosses `max_base` mid-block (the fill boundary) and
    // odd `max_base - base` remainders exercise the scalar-exact
    // `ceil` lane count.
    let mut seed = 0x1357u32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    let origin = EDGE_ORIGIN;
    for &bw in &[4usize, 8, 16, 32] {
        for &bh in &[4usize, 8, 16, 32, 64] {
            // 256 > EDGE_BUF_LEN: `max_base` reaches
            // `(32 + 64 - 1) << 1 = 190` under upsample, and the
            // scalar core (like C) indexes up to `origin + max_base`
            // unconditionally. Production can't generate that —
            // `svt_aom_use_intra_edge_upsample` requires
            // `bw + bh <= 16` — but the sweep covers it, so the
            // buffer needs the headroom C's stack slack gave it.
            let above: Vec<u8> = (0..256).map(|_| next()).collect();
            for &dx in &[1023i32, 547, 372, 273, 151, 90, 64, 45, 27, 15, 7, 3] {
                for &ua in &[false, true] {
                    let mut want = vec![0u8; bw * bh];
                    let mut got = vec![0u8; bw * bh];
                    dr_z1_edged_core(&mut want, bw, bw, bh, &above, origin, ua, dx);
                    dr_z1_edged(&mut got, bw, bw, bh, &above, origin, ua, dx);
                    if want != got {
                        let i = want.iter().zip(&got).position(|(a, b)| a != b).unwrap();
                        panic!(
                            "dr_z1_edged {bw}x{bh} dx={dx} ua={ua} \
                                 diverges at ({}, {}): want {} got {}",
                            i / bw,
                            i % bw,
                            want[i],
                            got[i]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn dr_z3_edged_dispatch_matches_core_all_sizes_flags_angles() {
    // dr_z3_edged's arms — the shared small kernel + transposed
    // store (bh 4/8) and the >= 16-row flat NEON — against the
    // scalar core. Same legal derivative set as the z1 sweep.
    let mut seed = 0x8642u32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    let origin = EDGE_ORIGIN;
    for &bw in &[4usize, 8, 16, 32, 64] {
        for &bh in &[4usize, 8, 16, 32] {
            // 256-byte buffer for the same reason the z1 sweep notes:
            // upsampled `max_base` reaches `(64 + 8 - 1) << 1 = 142`.
            let left: Vec<u8> = (0..256).map(|_| next()).collect();
            for &dy in &[1023i32, 547, 372, 273, 151, 90, 64, 45, 27, 15, 7, 3] {
                for &ul in &[false, true] {
                    let mut want = vec![0u8; bw * bh];
                    let mut got = vec![0u8; bw * bh];
                    dr_z3_edged_core(&mut want, bw, bw, bh, &left, origin, ul, dy);
                    dr_z3_edged(&mut got, bw, bw, bh, &left, origin, ul, dy);
                    if want != got {
                        let i = want.iter().zip(&got).position(|(a, b)| a != b).unwrap();
                        panic!(
                            "dr_z3_edged {bw}x{bh} dy={dy} ul={ul} \
                                 diverges at ({}, {}): want {} got {}",
                            i / bw,
                            i % bw,
                            want[i],
                            got[i]
                        );
                    }
                }
            }
        }
    }
}

/// Same sweep under every token permutation — the v3 arm and the scalar
/// arm both run, and a `width % 8 != 0` shape (e.g. width 4) confirms the
/// arms' gate still hands off to the core.
#[test]
fn predict_smooth_vh_dispatch_matches_core_all_sizes() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut seed = 0x6789au32;
    let mut next = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) as u8
    };
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
        for &width in &[4usize, 8, 16, 32, 64] {
            for &height in &[4usize, 8, 16, 32, 64] {
                let above: Vec<u8> = (0..width).map(|_| next()).collect();
                let left: Vec<u8> = (0..height).map(|_| next()).collect();
                let mut want = vec![0u8; width * height];
                let mut got = vec![0u8; width * height];
                predict_smooth_v_core(&mut want, width, &above, &left, height, width);
                predict_smooth_v(&mut got, width, &above, &left, 0, height, width);
                assert_eq!(want, got, "predict_smooth_v {width}x{height} tier {perm}");
                predict_smooth_h_core(&mut want, width, &above, &left, width, height);
                predict_smooth_h(&mut got, width, &above, &left, width, height);
                assert_eq!(want, got, "predict_smooth_h {width}x{height} tier {perm}");
            }
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2, "{report:?}");
}

#[test]
fn dc_above_only() {
    let above = [200u8; 8];
    let left = [0u8; 8];
    let mut dst = [0u8; 64];
    predict_dc(&mut dst, 8, &above, &left, 8, 8, true, false);
    assert!(dst.iter().all(|&v| v == 200));
}

#[test]
fn dc_no_neighbors() {
    let mut dst = [0u8; 16];
    predict_dc(&mut dst, 4, &[], &[], 4, 4, false, false);
    assert!(dst.iter().all(|&v| v == 128));
}

#[test]
fn v_pred_copies_above() {
    let above = [10u8, 20, 30, 40];
    let mut dst = [0u8; 16];
    predict_v(&mut dst, 4, &above, 4, 4);
    for row in 0..4 {
        assert_eq!(&dst[row * 4..row * 4 + 4], &above);
    }
}

#[test]
fn h_pred_copies_left() {
    let left = [10u8, 20, 30, 40];
    let mut dst = [0u8; 16];
    predict_h(&mut dst, 4, &left, 4, 4);
    for row in 0..4 {
        assert!(dst[row * 4..row * 4 + 4].iter().all(|&v| v == left[row]));
    }
}

#[test]
fn paeth_uniform() {
    // When above, left, and top_left are all the same, paeth should produce that value
    let above = [128u8; 4];
    let left = [128u8; 4];
    let mut dst = [0u8; 16];
    predict_paeth(&mut dst, 4, &above, &left, 128, 4, 4);
    assert!(dst.iter().all(|&v| v == 128));
}

#[test]
fn paeth_horizontal_gradient() {
    // Top-left = 0, above = [10,20,30,40], left = [0,0,0,0]
    // base = above[c] + left[r] - tl = above[c], so pred = above[c]
    let above = [10u8, 20, 30, 40];
    let left = [0u8; 4];
    let mut dst = [0u8; 16];
    predict_paeth(&mut dst, 4, &above, &left, 0, 4, 4);
    for row in 0..4 {
        for col in 0..4 {
            assert_eq!(dst[row * 4 + col], above[col]);
        }
    }
}

#[test]
fn smooth_corners() {
    // Smooth prediction should interpolate between neighbors
    let above = [200u8; 4];
    let left = [200u8; 4];
    let mut dst = [0u8; 16];
    predict_smooth(&mut dst, 4, &above, &left, 4, 4);
    // All neighbors are 200, so prediction should be 200
    for &v in &dst {
        assert!((v as i32 - 200).abs() <= 1, "expected ~200, got {v}");
    }
}

#[test]
fn smooth_v_interpolates() {
    let above = [255u8; 4];
    let left = [255, 255, 255, 0]; // bottom pixel is 0
    let mut dst = [0u8; 16];
    predict_smooth_v(&mut dst, 4, &above, &left, 0, 4, 4);
    // First row should be close to 255 (high weight on above)
    assert!(dst[0] > 200);
    // Last row should be closer to 0 (low weight on above)
    assert!(dst[12] < dst[0]);
}

#[test]
fn smooth_h_interpolates() {
    let above = [255, 255, 255, 0]; // right pixel is 0
    let left = [255u8; 4];
    let mut dst = [0u8; 16];
    predict_smooth_h(&mut dst, 4, &above, &left, 4, 4);
    // First column should be close to 255
    assert!(dst[0] > 200);
    // Last column should be closer to 0
    assert!(dst[3] < dst[0]);
}

// =========================================================================
// Directional kernel exact-value tests vs libaom av1_dr_prediction_z*_c
// (av1/common/reconintra.c, upsample = 0). Expected values are
// hand-computed from the C formulas — the same math the AV1 reference
// decoder runs, which is what the recon-parity gate compares against.
// =========================================================================

#[test]
fn dr_z1_d45_exact_diagonal() {
    // D45: dx = DR_INTRA_DERIVATIVE[45] = 64.
    // x = 64*(r+1) → base = r+1, shift = 0 → pred[r][c] = above[r+c+1]
    // while r+c+1 < max_base_x (= 7 for 4x4), else above[max_base_x].
    let above: Vec<u8> = (0..8).map(|i| (10 + i * 10) as u8).collect();
    let left = [0u8; 8];
    let mut dst = [0u8; 16];
    predict_directional(&mut dst, 4, &above, &left, 99, 4, 4, 45);
    for r in 0..4 {
        for c in 0..4 {
            let idx = (r + c + 1).min(7);
            assert_eq!(dst[r * 4 + c], above[idx], "D45 exact at ({r},{c})");
        }
    }
}

#[test]
fn dr_z2_d135_exact_diagonal() {
    // D135: dx = dy = DR_INTRA_DERIVATIVE[45] = 64. All shifts are 0, so
    // the prediction is the pure -45° diagonal through the top-left:
    //   pred[r][c] = above[c-r-1]  (c > r)
    //              = top_left      (c == r)
    //              = left[r-c-1]   (c < r)
    let above = [10u8, 20, 30, 40];
    let left = [50u8, 60, 70, 80];
    let top_left = 100u8;
    let mut dst = [0u8; 16];
    predict_directional(&mut dst, 4, &above, &left, top_left, 4, 4, 135);
    for r in 0..4 {
        for c in 0..4 {
            let expected = match c.cmp(&r) {
                core::cmp::Ordering::Greater => above[c - r - 1],
                core::cmp::Ordering::Equal => top_left,
                core::cmp::Ordering::Less => left[r - c - 1],
            };
            assert_eq!(dst[r * 4 + c], expected, "D135 exact at ({r},{c})");
        }
    }
}

#[test]
fn dr_z2_d113_exact_4x4() {
    // D113: dx = DR_INTRA_DERIVATIVE[180-113] = deriv[67] = 27,
    //        dy = DR_INTRA_DERIVATIVE[113-90]  = deriv[23] = 151.
    // Full 4x4 hand-computed from av1_dr_prediction_z2_c with
    // above = [10,20,30,40], left = [50,60,70,80], above[-1] = 100.
    // e.g. r0c0: x = -27, base_x = -1, shift = 18 →
    //      (100*14 + 10*18 + 16) >> 5 = 49.
    //      r2c0: x = -81, base_x = -2 → left branch: y = 128 - 151 =
    //      -23, base_y = -1, shift = 20 →
    //      (100*12 + 50*20 + 16) >> 5 = 69.
    let above = [10u8, 20, 30, 40];
    let left = [50u8, 60, 70, 80];
    let top_left = 100u8;
    let mut dst = [0u8; 16];
    predict_directional(&mut dst, 4, &above, &left, top_left, 4, 4, 113);
    let expected: [u8; 16] = [
        49, 16, 26, 36, //
        86, 12, 22, 32, //
        69, 35, 17, 27, //
        56, 72, 13, 23,
    ];
    assert_eq!(dst, expected, "D113 exact 4x4");
}

#[test]
fn dr_z2_d157_exact_spots() {
    // D157: dx = deriv[180-157] = deriv[23] = 151,
    //        dy = deriv[157-90]  = deriv[67] = 27.
    let above = [10u8, 20, 30, 40];
    let left = [50u8, 60, 70, 80];
    let top_left = 100u8;
    let mut dst = [0u8; 16];
    predict_directional(&mut dst, 4, &above, &left, top_left, 4, 4, 157);
    // r0c0: x = -151, base_x = -3 → left branch: y = -27, base_y = -1,
    //       shift = 18 → (100*14 + 50*18 + 16) >> 5 = 72
    assert_eq!(dst[0], 72, "D157 r0c0");
    // r0c1: x = -87, base_x = -2 → left: y = -54, base_y = -1,
    //       shift = 5 → (100*27 + 50*5 + 16) >> 5 = 92
    assert_eq!(dst[1], 92, "D157 r0c1");
    // r0c2: x = -23, base_x = -1, shift = 20 →
    //       (100*12 + 10*20 + 16) >> 5 = 44
    assert_eq!(dst[2], 44, "D157 r0c2");
    // r0c3: x = 41, base_x = 0, shift = 20 →
    //       (10*12 + 20*20 + 16) >> 5 = 16
    assert_eq!(dst[3], 16, "D157 r0c3");
    // r1c0: x = -302, base_x = -5 → left: y = 64 - 27 = 37,
    //       base_y = 0, shift = 18 → (50*14 + 60*18 + 16) >> 5 = 56
    assert_eq!(dst[4], 56, "D157 r1c0");
}

#[test]
fn dr_z3_d203_exact_spots() {
    // D203: dy = DR_INTRA_DERIVATIVE[270-203] = deriv[67] = 27.
    let above = [0u8; 8];
    let left: Vec<u8> = (0..8).map(|i| (50 + i * 10) as u8).collect();
    let mut dst = [0u8; 16];
    predict_directional(&mut dst, 4, &above, &left, 99, 4, 4, 203);
    // c0: y = 27, base = 0, shift = 13:
    //   r0: (50*19 + 60*13 + 16) >> 5 = 54
    //   r1: (60*19 + 70*13 + 16) >> 5 = 64
    assert_eq!(dst[0], 54, "D203 r0c0");
    assert_eq!(dst[4], 64, "D203 r1c0");
    // c1: y = 54, base = 0, shift = 27:
    //   r0: (50*5 + 60*27 + 16) >> 5 = 58
    assert_eq!(dst[1], 58, "D203 r0c1");
}

#[test]
fn filter_intra_mode0_4x4() {
    // above[0] = top_left, above[1..5] = above pixels
    let above = [100u8, 110, 120, 130, 140];
    let left = [90u8, 80, 70, 60];
    let mut dst = [0u8; 16];
    predict_filter_intra(&mut dst, 4, &above, &left, 4, 4, 0);
    // Output should be non-zero
    for &v in &dst {
        assert!(v > 0, "filter-intra produced zero pixel");
    }
    // At least some variation in the output
    let min = dst.iter().copied().min().unwrap();
    let max = dst.iter().copied().max().unwrap();
    assert!(
        max > min,
        "filter-intra produced flat output, expected variation"
    );
}

#[test]
fn filter_intra_zero_neighbors() {
    // All neighbors at 128 should produce output close to 128
    let above = [128u8; 5]; // top_left + 4 above
    let left = [128u8; 4];
    let mut dst = [0u8; 16];
    predict_filter_intra(&mut dst, 4, &above, &left, 4, 4, 0);
    for (i, &v) in dst.iter().enumerate() {
        assert!(
            (v as i32 - 128).abs() <= 1,
            "pixel {i}: expected ~128, got {v}"
        );
    }
}

#[test]
fn filter_intra_all_modes_4x4() {
    // Verify all 5 modes produce valid, distinct output
    let above = [100u8, 110, 120, 130, 140];
    let left = [90u8, 80, 70, 60];
    let mut outputs = [[0u8; 16]; 5];
    for mode in 0..5u8 {
        predict_filter_intra(&mut outputs[mode as usize], 4, &above, &left, 4, 4, mode);
    }
    // At least some modes should differ
    let mut any_differ = false;
    for i in 1..5 {
        if outputs[i] != outputs[0] {
            any_differ = true;
            break;
        }
    }
    assert!(
        any_differ,
        "all 5 filter-intra modes produced identical output"
    );
}

#[test]
fn filter_intra_8x8() {
    // Larger block size
    let above = [128u8; 9]; // top_left + 8 above
    let left = [128u8; 8];
    let mut dst = [0u8; 64];
    predict_filter_intra(&mut dst, 8, &above, &left, 8, 8, 2);
    for &v in &dst {
        assert!(
            (v as i32 - 128).abs() <= 1,
            "expected ~128 for uniform input, got {v}"
        );
    }
}

/// `predict_filter_intra` under every dispatch tier must equal the scalar
/// core — over all 5 modes and every legal block dim, across flat,
/// extreme and pseudo-random neighbor patterns (the cases that stress
/// the maddubs sign/saturation and the mulhrs-vs-explicit-rounding
/// divergence on negative sums). The report is consumed, matching the
/// `for_each_tier` contract `cdef.rs`'s tests document.
#[test]
fn filter_intra_all_tiers_match_scalar() {
    use archmage::testing::{CompileTimePolicy, TokenPermutation, for_each_token_permutation};
    let mut check = |_: &TokenPermutation| {
        let mut st = 0x9E3779B97F4A7C15u64;
        let mut lcg = || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            (st >> 33) as u8
        };
        for kind in 0..4 {
            for w in [4usize, 8, 16, 32] {
                for h in [4usize, 8, 16, 32] {
                    let above: Vec<u8> = (0..w + 1)
                        .map(|i| match kind {
                            0 => 128,
                            1 => 255,
                            2 => ((i * 37 + w) & 0xFF) as u8,
                            _ => lcg(),
                        })
                        .collect();
                    let left: Vec<u8> = (0..h)
                        .map(|i| match kind {
                            0 => 128,
                            1 => 0,
                            2 => ((i * 53 + h) & 0xFF) as u8,
                            _ => lcg(),
                        })
                        .collect();
                    for mode in 0..5u8 {
                        let mut want = vec![0u8; w * h];
                        predict_filter_intra_core(&mut want, w, &above, &left, w, h, mode);
                        let mut got = vec![0u8; w * h];
                        predict_filter_intra(&mut got, w, &above, &left, w, h, mode);
                        assert_eq!(
                            got, want,
                            "kind={kind} {w}x{h} mode={mode}: dispatch diverged \
                                 from scalar"
                        );
                    }
                }
            }
        }
    };
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, &mut check);
    assert!(
        report.warnings.is_empty(),
        "archmage excluded token(s): {:?}",
        report.warnings
    );
    assert!(
        report.permutations_run >= 2,
        "tier sweep ran {} permutation(s) — cannot catch a SIMD-vs-scalar \
             divergence.",
        report.permutations_run
    );
}

#[test]
fn palette_basic() {
    let palette = [10u8, 50, 100, 200];
    // Color map: 4x4 block with indices into palette
    let color_map = [0u8, 1, 2, 3, 3, 2, 1, 0, 0, 0, 3, 3, 1, 1, 2, 2];
    let mut dst = [0u8; 16];
    predict_palette(&mut dst, 4, &color_map, 4, &palette, 4, 4);

    // Verify each pixel matches palette lookup
    let expected = [
        10u8, 50, 100, 200, 200, 100, 50, 10, 10, 10, 200, 200, 50, 50, 100, 100,
    ];
    assert_eq!(dst, expected);
}

#[test]
fn palette_single_color() {
    let palette = [42u8];
    let color_map = [0u8; 16];
    let mut dst = [0u8; 16];
    predict_palette(&mut dst, 4, &color_map, 4, &palette, 4, 4);
    assert!(dst.iter().all(|&v| v == 42));
}

#[test]
fn palette_with_stride() {
    let palette = [10u8, 20, 30];
    // Map stride larger than width (2x2 block in a 4-wide map)
    let color_map = [
        0u8, 1, 255, 255, // row 0 (only first 2 used)
        2, 0, 255, 255, // row 1
    ];
    let mut dst = [0u8; 8]; // 2x2 block with stride 4
    predict_palette(&mut dst, 4, &color_map, 4, &palette, 2, 2);
    assert_eq!(dst[0], 10); // palette[0]
    assert_eq!(dst[1], 20); // palette[1]
    assert_eq!(dst[4], 30); // palette[2]
    assert_eq!(dst[5], 10); // palette[0]
}
