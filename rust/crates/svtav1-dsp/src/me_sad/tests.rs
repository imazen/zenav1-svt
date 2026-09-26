use super::*;
use alloc::vec::Vec;
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

fn plane(seed: u32, n: usize) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        })
        .collect()
}

/// Every (w, h) the ME search actually uses, plus the odd widths that
/// exercise the 8-wide remainder and the scalar tail.
const SIZES: &[(usize, usize)] = &[
    (4, 4),
    (8, 4),
    (8, 8),
    (16, 8),
    (16, 16),
    (24, 16),
    (32, 32),
    (48, 32),
    (64, 64),
    (128, 64),
    (128, 128),
    (12, 6),
    (20, 3),
    (5, 7),
];

#[test]
fn four_candidate_sad_matches_independent_sums_all_tiers() {
    for &(w, h) in SIZES
        .iter()
        .chain(&[(4, 128), (128, 128), (1, 1), (7, 13), (31, 9)])
    {
        let ss = w + 7;
        let rs = w + 11;
        // The last row has exactly w samples: catch SIMD overreads.
        let src = plane(71, ss * (h - 1) + w);
        let refs: [Vec<u8>; 4] = core::array::from_fn(|i| plane(99 + i as u32, rs * (h - 1) + w));
        let refs = refs.each_ref().map(|v| v.as_slice());
        let expected: [u32; 4] = core::array::from_fn(|i| {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (y, x)))
                .map(|(y, x)| u32::from(src[y * ss + x].abs_diff(refs[i][y * rs + x])))
                .sum()
        });
        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
            assert_eq!(block_sad_x4(&src, ss, refs, rs, w, h), expected, "{w}x{h}");
        });
        assert!(report.warnings.is_empty(), "{report:?}");
    }
}

#[test]
fn me_sad_all_tiers_agree() {
    let stride = 160usize;
    let src = plane(7, stride * 160);
    let rf = plane(1_234_567, stride * 160);
    for &(w, h) in SIZES {
        let want = block_sad_scalar(
            ScalarToken::summon().unwrap(),
            &src,
            stride,
            &rf,
            stride,
            w,
            h,
        );
        // Independent recomputation, so the reference is not the arm.
        let mut check = 0u32;
        for y in 0..h {
            for x in 0..w {
                check += u32::from(src[y * stride + x].abs_diff(rf[y * stride + x]));
            }
        }
        assert_eq!(want, check, "scalar {w}x{h}");

        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
            assert_eq!(
                block_sad(&src, stride, &rf, stride, w, h),
                want,
                "tier mismatch at {w}x{h}"
            );
        });
        assert!(report.warnings.is_empty(), "excluded tokens: {report:?}");
        assert!(
            report.permutations_run >= 2,
            "no dispatch coverage: {report:?}"
        );
    }
}

#[test]
fn me_sad_handles_distinct_strides_and_offsets() {
    let (ss, rs) = (137usize, 96usize);
    let src = plane(99, ss * 140);
    let rf = plane(4_242, rs * 140);
    for &(w, h) in SIZES {
        let mut want = 0u32;
        for y in 0..h {
            for x in 0..w {
                want += u32::from(src[y * ss + x].abs_diff(rf[y * rs + x]));
            }
        }
        assert_eq!(block_sad(&src, ss, &rf, rs, w, h), want, "{w}x{h}");
    }
}

#[test]
fn me_sum_sse_all_tiers_agree() {
    let (as_, bs) = (137usize, 96usize);
    let a = plane(31, as_ * 140);
    let b = plane(9_871, bs * 140);
    for &(w, h) in SIZES {
        let mut sum = 0i32;
        let mut sse = 0u32;
        for y in 0..h {
            for x in 0..w {
                let d = i32::from(a[y * as_ + x]) - i32::from(b[y * bs + x]);
                sum += d;
                sse += (d * d) as u32;
            }
        }
        let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
            assert_eq!(block_sum_sse(&a, as_, &b, bs, w, h), (sum, sse), "{w}x{h}");
        });
        assert!(report.warnings.is_empty(), "excluded tokens: {report:?}");
        assert!(
            report.permutations_run >= 2,
            "no dispatch coverage: {report:?}"
        );
    }
}

/// The u32 `sse` accumulator's worst case is `255^2 * 128 * 128`, which is
/// 1_065_369_600 — inside u32 but only by 4x, so it is asserted rather
/// than argued.
#[test]
fn me_sum_sse_extremes() {
    let a = [0u8; 128 * 128];
    let b = [255u8; 128 * 128];
    assert_eq!(
        block_sum_sse(&a, 128, &b, 128, 128, 128),
        (-(255 * 128 * 128), 255 * 255 * 128 * 128)
    );
    assert_eq!(
        block_sum_sse(&b, 128, &a, 128, 128, 128),
        (255 * 128 * 128, 255 * 255 * 128 * 128)
    );
}

#[test]
fn me_sad_zero_and_max() {
    let a = [0u8; 128 * 8];
    let b = [255u8; 128 * 8];
    assert_eq!(block_sad(&a, 128, &a, 128, 128, 8), 0);
    assert_eq!(block_sad(&a, 128, &b, 128, 128, 8), 255 * 128 * 8);
}

/// Positive witness that the AVX-512 arms are EXERCISED, not just
/// compiled — the permutation tests above pass even if dispatch silently
/// falls through to `_v3`. On a host without AVX-512 the summon returns
/// `None` and the test reports the gap rather than claiming coverage.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[test]
fn me_sad_v4_arms_match_scalar_when_summoned() {
    extern crate std;
    let Some(v4) = X64V4Token::summon() else {
        std::eprintln!("X64V4Token unavailable — _v4 arms NOT exercised on this host");
        return;
    };
    let stride = 160usize;
    let src = plane(7, stride * 160);
    let rf = plane(1_234_567, stride * 160);
    for &(w, h) in SIZES {
        let want = block_sad_scalar(
            ScalarToken::summon().unwrap(),
            &src,
            stride,
            &rf,
            stride,
            w,
            h,
        );
        assert_eq!(
            block_sad_v4(v4, &src, stride, &rf, stride, w, h),
            want,
            "block_sad_v4 {w}x{h}"
        );
        let (sum, sse) = {
            let mut sum = 0i32;
            let mut sse = 0u32;
            for y in 0..h {
                for x in 0..w {
                    let d = i32::from(src[y * stride + x]) - i32::from(rf[y * stride + x]);
                    sum += d;
                    sse += (d * d) as u32;
                }
            }
            (sum, sse)
        };
        assert_eq!(
            block_sum_sse_v4(v4, &src, stride, &rf, stride, w, h),
            (sum, sse),
            "block_sum_sse_v4 {w}x{h}"
        );
        let ss = w + 7;
        let rs = w + 11;
        let src4 = plane(71, ss * (h - 1) + w);
        let refs4: [Vec<u8>; 4] = core::array::from_fn(|i| plane(99 + i as u32, rs * (h - 1) + w));
        let refs4 = refs4.each_ref().map(|v| v.as_slice());
        let expected: [u32; 4] = core::array::from_fn(|i| {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (y, x)))
                .map(|(y, x)| u32::from(src4[y * ss + x].abs_diff(refs4[i][y * rs + x])))
                .sum()
        });
        assert_eq!(
            block_sad_x4_v4(v4, &src4, ss, refs4, rs, w, h),
            expected,
            "block_sad_x4_v4 {w}x{h}"
        );
    }
    // Heights not divisible by the pack factors (4 and 2) hit the row
    // tails inside the packed width paths.
    for &(w, h) in &[(8, 5), (16, 7), (32, 3), (8, 1), (16, 1), (32, 1)] {
        let want = block_sad_scalar(
            ScalarToken::summon().unwrap(),
            &src,
            stride,
            &rf,
            stride,
            w,
            h,
        );
        assert_eq!(
            block_sad_v4(v4, &src, stride, &rf, stride, w, h),
            want,
            "block_sad_v4 tail {w}x{h}"
        );
    }
}
