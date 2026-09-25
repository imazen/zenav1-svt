use super::*;
use alloc::vec;
use alloc::vec::Vec;

/// Scalar reference routing for [`dr_predictor_edged_hbd`]: the private
/// cores directly, so every tier of the dispatched entry is compared
/// against the C-translated scalar bodies (z1/z3 flat NEON arms vs the
/// same core the non-gated shapes still take).
#[allow(clippy::too_many_arguments)]
fn dr_edged_ref(
    dst: &mut [u16],
    dst_stride: usize,
    above: &[u16],
    left: &[u16],
    origin: usize,
    upsample_above: bool,
    upsample_left: bool,
    width: usize,
    height: usize,
    angle: i32,
    bd: u8,
) {
    let dx = get_dx_hbd(angle);
    let dy = get_dy_hbd(angle);
    if angle > 0 && angle < 90 {
        dr_z1_edged_hbd_core(
            dst,
            dst_stride,
            width,
            height,
            above,
            origin,
            upsample_above,
            dx,
            bd,
        );
    } else if angle > 90 && angle < 180 {
        dr_z2_edged_hbd_core(
            dst,
            dst_stride,
            width,
            height,
            above,
            left,
            origin,
            upsample_above,
            upsample_left,
            dx,
            dy,
            bd,
        );
    } else if angle > 180 && angle < 270 {
        dr_z3_edged_hbd_core(
            dst,
            dst_stride,
            width,
            height,
            left,
            origin,
            upsample_left,
            dy,
            bd,
        );
    } else {
        panic!("test only sweeps z1/z2/z3");
    }
}

/// Every dispatched tier of `dr_predictor_edged_hbd` must produce the
/// scalar core's output on every size/angle/upsample/bd combination —
/// including the gated flat arms (bw/bh >= 8, non-upsampled) and the
/// shapes that still fall through to scalar. Consumes the
/// `PermutationReport` (empty warnings, >= 2 permutations).
#[test]
fn dr_edged_hbd_z1_z3_all_tiers_match_core() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut st = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u16
    };
    // Edge buffers are 512-deep: upsampled reads can reach past
    // EDGE_BUF_LEN on oversized shapes (the same harmless over-read the
    // u8 sibling and C perform), so the test buffers carry headroom.
    const BUF: usize = 512;
    const ORIGIN: usize = 16;
    // z1 angles with nonzero DR_INTRA_DERIVATIVE_HBD entries, plus the
    // z3 mirrors (270 - a) and z2 angles spanning shallow to steep
    // slopes. Real AV1 mode angles, not synthetic extremes.
    let z1_angles = [3i32, 9, 26, 45, 57, 72, 84, 87];
    // Only angles where BOTH 180-a and a-90 have nonzero derivative
    // entries are legal z2 modes (dx=0/dy=0 is contract-invalid).
    let z2_angles = [93i32, 99, 126, 132, 135, 141, 144, 174, 177];
    let z3_angles = [183i32, 189, 201, 219, 237, 255, 264, 267];
    let sizes = [
        (4usize, 4usize),
        (8, 4),
        (4, 8),
        (8, 8),
        (8, 16),
        (16, 8),
        (16, 16),
        (32, 8),
        (8, 32),
        (32, 32),
        (64, 16),
        (16, 64),
        (64, 64),
    ];
    for &bd in &[8u8, 10, 12] {
        let max_sample = (1u16 << bd.min(12)) - 1;
        for &(w, h) in &sizes {
            for &up in &[(false, false), (true, false), (false, true), (true, true)] {
                let above: Vec<u16> = (0..BUF).map(|_| next() & max_sample).collect();
                let left: Vec<u16> = (0..BUF).map(|_| next() & max_sample).collect();
                for &angle in z1_angles
                    .iter()
                    .chain(z2_angles.iter())
                    .chain(z3_angles.iter())
                {
                    for pad in [0usize, 3] {
                        let stride = w + pad;
                        let rep =
                            for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                                let mut got = vec![0xAAAAu16; stride * h];
                                let mut want = vec![0xBBBBu16; stride * h];
                                dr_predictor_edged_hbd(
                                    &mut got, stride, &above, &left, ORIGIN, up.0, up.1, w, h,
                                    angle, bd,
                                );
                                dr_edged_ref(
                                    &mut want, stride, &above, &left, ORIGIN, up.0, up.1, w, h,
                                    angle, bd,
                                );
                                for r in 0..h {
                                    assert_eq!(
                                        &got[r * stride..r * stride + w],
                                        &want[r * stride..r * stride + w],
                                        "dr hbd {w}x{h} angle {angle} up {up:?} bd {bd} \
                                             stride {stride} row {r} tier {perm}"
                                    );
                                }
                            });
                        assert!(
                            rep.warnings.is_empty(),
                            "tokens excluded at compile time: {:?}",
                            rep.warnings
                        );
                        assert!(
                            rep.permutations_run >= 2,
                            "only {} permutation(s) ran",
                            rep.permutations_run
                        );
                    }
                }
            }
        }
    }
}

/// The dispatched `predict_dc_hbd` must equal `predict_dc_hbd_core` on
/// every size, flag combination and stride — the u8 sibling's
/// `predict_dc_dispatch_matches_core_all_sizes_flags` for the u16 arm.
#[test]
fn predict_dc_hbd_dispatch_matches_core_all_sizes_flags() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut st = 0xDEAD_BEEF_CAFE_F00Du64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u16
    };
    for &bd in &[8u8, 10, 12] {
        let max_sample = (1u16 << bd.min(12)) - 1;
        for &(w, h) in &[
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 32),
            (64, 64),
            (4, 8),
            (8, 4),
            (16, 8),
            (8, 16),
            (32, 8),
            (64, 16),
            (16, 64),
        ] {
            for &(ha, hl) in &[(true, true), (true, false), (false, true), (false, false)] {
                let above: Vec<u16> = (0..w).map(|_| next() & max_sample).collect();
                let left: Vec<u16> = (0..h).map(|_| next() & max_sample).collect();
                for pad in [0usize, 5] {
                    let stride = w + pad;
                    let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                        let mut got = vec![0xAAAAu16; stride * h];
                        let mut want = vec![0xBBBBu16; stride * h];
                        predict_dc_hbd(&mut got, stride, &above, &left, w, h, ha, hl, bd);
                        predict_dc_hbd_core(&mut want, stride, &above, &left, w, h, ha, hl, bd);
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "dc hbd {w}x{h} flags ({ha},{hl}) bd {bd} stride {stride} \
                                         row {r} tier {perm}"
                            );
                        }
                    });
                    assert!(
                        rep.warnings.is_empty(),
                        "tokens excluded at compile time: {:?}",
                        rep.warnings
                    );
                    assert!(
                        rep.permutations_run >= 2,
                        "only {} permutation(s) ran",
                        rep.permutations_run
                    );
                }
            }
        }
    }
}

/// Every dispatched tier of the hbd paeth/smooth/filter-intra/CfL arms
/// must produce the scalar core's output — including the non-multiple-of-4
/// paeth widths that exercise the scalar tails (6, 10, 18 are not real
/// AV1 sizes but the paeth kernel is generic and the tail is load-bearing).
/// Consumes the `PermutationReport` (empty warnings, >= 2 permutations).
#[test]
fn hbd_predictors_all_tiers_match_core() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let scalar = ScalarToken::summon().unwrap();
    let mut st = 0x243F_6A88_85A3_08D3u64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u16
    };
    // Paeth has no weight tables — generic widths (6/10/18) exercise the
    // NEON tails. The smooth predictors index `smooth_weights_hbd(n)`
    // which only exists for real AV1 sizes {4,8,16,32,64}.
    for &bd in &[10u8, 12, 8] {
        let max_sample = (1u16 << bd.min(12)) - 1;
        for &(w, h) in &[
            (4usize, 4usize),
            (6, 4),
            (8, 8),
            (10, 8),
            (16, 16),
            (18, 8),
            (32, 16),
            (4, 32),
        ] {
            let above: Vec<u16> = (0..w + 1).map(|_| next() & max_sample).collect();
            let left: Vec<u16> = (0..h.max(2)).map(|_| next() & max_sample).collect();
            for pad in [0usize, 3] {
                let stride = w + pad;
                let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                    let mut got = vec![0xAAAAu16; stride * h];
                    let mut want = vec![0xBBBBu16; stride * h];
                    let tl = next() & max_sample;
                    predict_paeth_hbd(&mut got, stride, &above[1..], &left, tl, w, h);
                    predict_paeth_hbd_impl_scalar(
                        scalar,
                        &mut want,
                        stride,
                        &above[1..],
                        &left,
                        tl,
                        w,
                        h,
                    );
                    for r in 0..h {
                        assert_eq!(
                            &got[r * stride..r * stride + w],
                            &want[r * stride..r * stride + w],
                            "paeth {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                        );
                    }
                });
                assert!(
                    rep.warnings.is_empty(),
                    "tokens excluded at compile time: {:?}",
                    rep.warnings
                );
                assert!(
                    rep.permutations_run >= 2,
                    "only {} permutation(s) ran",
                    rep.permutations_run
                );
            }
        }
        for &(w, h) in &[
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 16),
            (4, 32),
            (64, 8),
        ] {
            let above: Vec<u16> = (0..w).map(|_| next() & max_sample).collect();
            let left: Vec<u16> = (0..h).map(|_| next() & max_sample).collect();
            for pad in [0usize, 3] {
                let stride = w + pad;
                let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                    let mut got = vec![0xAAAAu16; stride * h];
                    let mut want = vec![0xBBBBu16; stride * h];

                    predict_smooth_hbd(&mut got, stride, &above, &left, w, h);
                    predict_smooth_hbd_core(&mut want, stride, &above, &left, w, h);
                    for r in 0..h {
                        assert_eq!(
                            &got[r * stride..r * stride + w],
                            &want[r * stride..r * stride + w],
                            "smooth {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                        );
                    }

                    got.fill(0xAAAA);
                    want.fill(0xBBBB);
                    predict_smooth_v_hbd(&mut got, stride, &above, &left, w, h);
                    predict_smooth_v_hbd_core(&mut want, stride, &above, &left, w, h);
                    for r in 0..h {
                        assert_eq!(
                            &got[r * stride..r * stride + w],
                            &want[r * stride..r * stride + w],
                            "smooth_v {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                        );
                    }

                    got.fill(0xAAAA);
                    want.fill(0xBBBB);
                    predict_smooth_h_hbd(&mut got, stride, &above, &left, w, h);
                    predict_smooth_h_hbd_core(&mut want, stride, &above, &left, w, h);
                    for r in 0..h {
                        assert_eq!(
                            &got[r * stride..r * stride + w],
                            &want[r * stride..r * stride + w],
                            "smooth_h {w}x{h} bd {bd} stride {stride} row {r} tier {perm}"
                        );
                    }
                });
                assert!(
                    rep.warnings.is_empty(),
                    "tokens excluded at compile time: {:?}",
                    rep.warnings
                );
                assert!(
                    rep.permutations_run >= 2,
                    "only {} permutation(s) ran",
                    rep.permutations_run
                );
            }
        }
    }

    // filter_intra: 4x2 sub-blocks, w/h <= 32 and multiples of 4, mode < 5.
    for &bd in &[10u8, 12] {
        let max_sample = (1u16 << bd.min(12)) - 1;
        for &(w, h) in &[
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 32),
            (4, 16),
            (20, 8),
        ] {
            for mode in 0u8..5 {
                let above: Vec<u16> = (0..w + 1).map(|_| next() & max_sample).collect();
                let left: Vec<u16> = (0..h).map(|_| next() & max_sample).collect();
                for pad in [0usize, 3] {
                    let stride = w + pad;
                    let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                        let mut got = vec![0xAAAAu16; stride * h];
                        let mut want = vec![0xBBBBu16; stride * h];
                        predict_filter_intra_hbd(&mut got, stride, &above, &left, w, h, mode, bd);
                        predict_filter_intra_hbd_core(
                            &mut want, stride, &above, &left, w, h, mode, bd,
                        );
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "filter_intra {w}x{h} mode {mode} bd {bd} stride \
                                         {stride} row {r} tier {perm}"
                            );
                        }
                    });
                    assert!(
                        rep.warnings.is_empty(),
                        "tokens excluded at compile time: {:?}",
                        rep.warnings
                    );
                    assert!(
                        rep.permutations_run >= 2,
                        "only {} permutation(s) ran",
                        rep.permutations_run
                    );
                }
            }
        }
    }

    // CfL hbd pair: subsampling (w,h even) and predict (alpha over the
    // signed q3 domain, incl. the bd=8 default-clip arm).
    for &(w, h) in &[
        (4usize, 4usize),
        (8, 8),
        (16, 16),
        (32, 32),
        (6, 4),
        (18, 8),
    ] {
        let luma: Vec<u16> = (0..h * w).map(|_| next() & 4095).collect();
        let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
            let mut got = vec![0i16; (h / 2 + 1) * crate::intra_pred::CFL_BUF_LINE + w];
            let mut want = vec![0i16; (h / 2 + 1) * crate::intra_pred::CFL_BUF_LINE + w];
            cfl_luma_subsampling_420_hbd(&luma, w, &mut got, w, h);
            cfl_luma_subsampling_420_hbd_core(&luma, w, &mut want, w, h);
            assert_eq!(got, want, "cfl_subsample {w}x{h} tier {perm}");
        });
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
        assert!(rep.permutations_run >= 2);
    }
    for &bd in &[10u8, 12, 8] {
        let max_sample = (1u16 << bd.min(12)) - 1;
        for &(w, h) in &[
            (4usize, 4usize),
            (8, 8),
            (16, 16),
            (32, 16),
            (6, 4),
            (10, 8),
        ] {
            let buf: Vec<i16> = (0..h * crate::intra_pred::CFL_BUF_LINE + w)
                .map(|_| (next() & 1023) as i16 - 512)
                .collect();
            let pred: Vec<u16> = (0..h * w).map(|_| next() & max_sample).collect();
            for &alpha in &[-16i32, -3, 0, 5, 16] {
                for pad in [0usize, 3] {
                    let stride = w + pad;
                    let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                        let mut got = vec![0xAAAAu16; stride * h];
                        let mut want = vec![0xBBBBu16; stride * h];
                        cfl_predict_hbd(&buf, &pred, w, &mut got, stride, alpha, bd, w, h);
                        cfl_predict_hbd_core(&buf, &pred, w, &mut want, stride, alpha, bd, w, h);
                        for r in 0..h {
                            assert_eq!(
                                &got[r * stride..r * stride + w],
                                &want[r * stride..r * stride + w],
                                "cfl_predict {w}x{h} alpha {alpha} bd {bd} stride \
                                         {stride} row {r} tier {perm}"
                            );
                        }
                    });
                    assert!(
                        rep.warnings.is_empty(),
                        "tokens excluded at compile time: {:?}",
                        rep.warnings
                    );
                    assert!(
                        rep.permutations_run >= 2,
                        "only {} permutation(s) ran",
                        rep.permutations_run
                    );
                }
            }
        }
    }
}
