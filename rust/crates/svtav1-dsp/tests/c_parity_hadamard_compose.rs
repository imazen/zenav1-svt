//! Large Hadamard composition: real AVX2 C semantics including i16 wrapping.
#![cfg(target_arch = "x86_64")]
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
use svtav1_dsp::hadamard;
#[test]
fn large_hadamard_padded_full_range_all_tiers_match_c() {
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        let mut seed = 987654321u32;
        let mut count = 0;
        for n in [16, 32] {
            for stride in [n, n + 7, n * 2] {
                for pattern in 0..100 {
                    let mut input = vec![0i16; stride * n + 3];
                    for (i, v) in input.iter_mut().enumerate() {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        *v = match pattern {
                            0 => 0,
                            1 => 255,
                            2 => -255,
                            3 => 1023,
                            4 => -1023,
                            5 => i16::MIN,
                            6 => i16::MAX,
                            7 => {
                                if i % 2 == 0 {
                                    i16::MIN
                                } else {
                                    i16::MAX
                                }
                            }
                            _ => seed as i16,
                        };
                    }
                    let mut c = vec![0; n * n];
                    let mut a = vec![777; n * n + 9];
                    svtav1_cref::hadamard_avx2(n, &input[3..], stride, &mut c);
                    if n == 16 {
                        hadamard::aom_hadamard_16x16(&input[3..], stride, &mut a[3..3 + n * n]);
                    } else {
                        hadamard::aom_hadamard_32x32(&input[3..], stride, &mut a[3..3 + n * n]);
                    }
                    assert_eq!(&a[..3], &[777; 3]);
                    assert_eq!(&a[3 + n * n..], &[777; 6]);
                    if pattern <= 2 {
                        let mut scalar = vec![0; n * n];
                        svtav1_cref::hadamard(n, &input[3..], stride, &mut scalar);
                        assert_eq!(
                            &a[3..3 + n * n],
                            scalar,
                            "scalar positional n={n} stride={stride} pattern={pattern}"
                        );
                    }
                    // C AVX2 permutes coefficients; the consuming SATD is order independent.
                    let mut sorted = a[3..3 + n * n].to_vec();
                    sorted.sort_unstable();
                    c.sort_unstable();
                    assert_eq!(
                        sorted, c,
                        "C multiset n={n} stride={stride} pattern={pattern}"
                    );
                    count += 1;
                }
            }
        }
        assert_eq!(count, 600);
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}
