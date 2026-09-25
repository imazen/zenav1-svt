use super::*;
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

#[test]
fn fwd_txfm2d_4x4_dct_dct_all_dispatch_levels() {
    let input: [i16; 16] = [
        10, -20, 30, -40, 50, -60, 70, -80, 15, -25, 35, -45, 55, -65, 75, -85,
    ];
    let mut reference = [0i32; 16];
    fwd_txfm2d_4x4_dct_dct(&input, &mut reference, 4);

    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
        let mut result = [0i32; 16];
        fwd_txfm2d_4x4_dct_dct(&input, &mut result, 4);
        assert_eq!(
            result, reference,
            "4x4 DCT mismatch at dispatch level {_perm}"
        );
    });
}

#[test]
fn fwd_txfm2d_8x8_dct_dct_all_dispatch_levels() {
    let mut input = [0i16; 64];
    for (i, v) in input.iter_mut().enumerate() {
        *v = ((i as i32 * 7 - 30) % 100) as i16;
    }
    let mut reference = [0i32; 64];
    fwd_txfm2d_8x8_dct_dct(&input, &mut reference, 8);

    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
        let mut result = [0i32; 64];
        fwd_txfm2d_8x8_dct_dct(&input, &mut result, 8);
        assert_eq!(
            result, reference,
            "8x8 DCT mismatch at dispatch level {_perm}"
        );
    });
}
