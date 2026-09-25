use super::*;
use alloc::vec;
use alloc::vec::Vec;
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

#[test]
fn paeth_all_dispatch_levels() {
    let above: Vec<u8> = (0..8).map(|i| (50 + i * 20) as u8).collect();
    let left: Vec<u8> = (0..8).map(|i| (200 - i * 15) as u8).collect();
    let top_left = 100u8;
    let mut ref_dst = vec![0u8; 64];
    predict_paeth(&mut ref_dst, 8, &above, &left, top_left, 8, 8);

    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
        let mut dst = vec![0u8; 64];
        predict_paeth(&mut dst, 8, &above, &left, top_left, 8, 8);
        assert_eq!(dst, ref_dst, "paeth mismatch at dispatch level");
    });
}

#[test]
fn paeth_dispatch_4x4() {
    let above = [10u8, 20, 30, 40];
    let left = [50u8, 60, 70, 80];
    let top_left = 5u8;
    let mut ref_dst = [0u8; 16];
    predict_paeth(&mut ref_dst, 4, &above, &left, top_left, 4, 4);

    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
        let mut dst = [0u8; 16];
        predict_paeth(&mut dst, 4, &above, &left, top_left, 4, 4);
        assert_eq!(dst, ref_dst, "paeth 4x4 mismatch at dispatch level");
    });
}

#[test]
fn paeth_dispatch_16x16() {
    let above: Vec<u8> = (0..16).map(|i| (i * 15) as u8).collect();
    let left: Vec<u8> = (0..16).map(|i| (255 - i * 12) as u8).collect();
    let top_left = 128u8;
    let mut ref_dst = vec![0u8; 256];
    predict_paeth(&mut ref_dst, 16, &above, &left, top_left, 16, 16);

    let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
        let mut dst = vec![0u8; 256];
        predict_paeth(&mut dst, 16, &above, &left, top_left, 16, 16);
        assert_eq!(dst, ref_dst, "paeth 16x16 mismatch at dispatch level");
    });
}
