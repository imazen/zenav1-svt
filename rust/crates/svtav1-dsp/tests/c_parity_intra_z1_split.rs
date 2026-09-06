//! Zone-one prediction: direct real C across dispatch tiers and fallback modes.
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
use svtav1_dsp::intra_pred;
#[test]
fn zone_one_all_tiers_match_c() {
    let angles = [
        3, 6, 9, 14, 17, 20, 23, 26, 29, 32, 36, 39, 42, 45, 48, 51, 54, 58, 61, 64, 67, 70, 73,
        76, 81, 84, 87,
    ];
    let report = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_| {
        let mut checked = 0;
        for (w, h) in [
            (4, 4),
            (8, 8),
            (16, 4),
            (16, 8),
            (16, 16),
            (32, 8),
            (32, 16),
            (32, 32),
            (64, 16),
            (64, 32),
            (64, 64),
        ] {
            for stride in [w, w + 7] {
                for origin in [3, 16] {
                    for up in [false, true] {
                        for angle in angles {
                            for pattern in 0..5 {
                                let edge: Vec<u8> = (0..origin + 2 * (w + h) + 1)
                                    .map(|i| match pattern {
                                        0 => 0,
                                        1 => 255,
                                        2 => {
                                            if i % 2 == 0 {
                                                0
                                            } else {
                                                255
                                            }
                                        }
                                        _ => ((i * 7919 + pattern * 997) % 256) as u8,
                                    })
                                    .collect();
                                let mut c = vec![77; stride * h + 9];
                                let mut a = c.clone();
                                svtav1_cref::dr_predictor_edged(
                                    &mut c[3..],
                                    stride,
                                    &edge,
                                    &edge,
                                    origin,
                                    up,
                                    false,
                                    w,
                                    h,
                                    angle,
                                );
                                intra_pred::dr_predictor_edged(
                                    &mut a[3..],
                                    stride,
                                    &edge,
                                    &edge,
                                    origin,
                                    up,
                                    false,
                                    w,
                                    h,
                                    angle,
                                );
                                assert_eq!(
                                    a, c,
                                    "{w}x{h} stride={stride} origin={origin} up={up} angle={angle} pattern={pattern}"
                                );
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 11880);
    });
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.permutations_run >= 2);
}
