//! SATD reductions: real C in its range, closed-form wider sums beyond i32.
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
use svtav1_dsp::hadamard;

#[test]
fn coefficient_sums_all_tiers_match_c_and_wide_edges() {
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        let mut seed = 7654321u32;
        let mut checked = 0;
        for n in [0, 1, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 65, 256, 1024] {
            for pattern in 0..32 {
                let input: Vec<i32> = (0..n + 3)
                    .map(|i| {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        match pattern {
                            0 => 0,
                            1 => 32640,
                            2 => -32640,
                            3 => {
                                if i % 2 == 0 {
                                    32640
                                } else {
                                    -32640
                                }
                            }
                            _ => (seed % 65281) as i32 - 32640,
                        }
                    })
                    .collect();
                let input = &input[3..];
                let expected = svtav1_cref::satd(input);
                assert_eq!(
                    hadamard::aom_satd(input),
                    expected,
                    "n={n} pattern={pattern}"
                );
                assert_eq!(
                    hadamard::sum_abs_coeffs_wide(input),
                    i64::from(expected),
                    "wide n={n} pattern={pattern}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 480);
        for n in [
            0, 1, 2, 3, 4, 7, 8, 15, 16, 17, 31, 32, 33, 63, 64, 65, 1023, 1024, 1025, 4096,
        ] {
            let mut input = vec![12345, 6789, -333];
            input.extend((0..n).map(|i| [i32::MIN, i32::MAX, -1, 0][i % 4]));
            let tail = [0, 2_147_483_648, 4_294_967_295, 4_294_967_296][n % 4];
            let expected = (n / 4) as i64 * 4_294_967_296 + tail;
            assert_eq!(
                hadamard::sum_abs_coeffs_wide(&input[3..]),
                expected,
                "wide n={n}"
            );
        }
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}
