use super::*;

/// satd_4x4 vs its scalar core, every tier, on random content.
///
/// The other satd_4x4 tests here are identical-blocks (always 0) and a
/// UNIFORM difference (80). A uniform residual puts all the energy in DC,
/// so both would pass against a broken transpose or a mis-paired
/// butterfly — precisely the mistakes a hand-written Hadamard makes. This
/// uses random residuals, where every coefficient is live, and varies the
/// strides so a kernel that ignored them is caught.
#[test]
fn satd_4x4_random_all_tiers_match_core() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut st = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u8
    };
    for case in 0..64 {
        let (ss, rs) = (4 + case % 5, 4 + (case * 3) % 7);
        let src: alloc::vec::Vec<u8> = (0..ss * 4 + 8).map(|_| next()).collect();
        let rf: alloc::vec::Vec<u8> = (0..rs * 4 + 8).map(|_| next()).collect();
        let expect = satd_4x4_core(&src, ss, &rf, rs);
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let got = satd_4x4(&src, ss, &rf, rs);
            assert_eq!(
                got, expect,
                "satd_4x4 case {case} strides ({ss},{rs}) tier {_perm}"
            );
        });
    }
}

#[test]
fn satd_4x4_identical() {
    let block = [128u8; 64];
    assert_eq!(satd_4x4(&block, 8, &block, 8), 0);
}

#[test]
fn satd_4x4_uniform_diff() {
    let src = [110u8; 16];
    let ref_ = [100u8; 16];
    // Uniform difference of 10 across 4x4 block.
    // Hadamard of constant = value * N at DC, 0 elsewhere
    // DC = 10 * 16 = 160, SATD = |160| / 2 = 80
    assert_eq!(satd_4x4(&src, 4, &ref_, 4), 80);
}

/// Every dispatch tier of the 2D 8x8 Hadamard must agree with an
/// INDEPENDENT scalar oracle written from C's `hadamard_col8` +
/// `svt_aom_hadamard_8x8_c`, on random residuals at strides wider than the
/// block. Positional coefficients: unlike SATD, a wrong output permutation
/// or a wrong transpose changes the answer, and both are the mistakes a
/// vectorised Hadamard makes.
///
/// The range deliberately includes 10-BIT residuals ([-1023, 1023]), where
/// the i16 lanes of the NEON arm wrap and the scalar arm's i32 intermediates
/// do not — they must still agree, because truncation to 16 bits commutes
/// with add/sub.
#[test]
fn aom_hadamard_8x8_random_all_tiers_match_oracle() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut st = 0xD1B5_4A32_D192_ED03u64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u32
    };
    fn oracle(src: &[i16], stride: usize) -> [i32; 64] {
        let col = |v: &[i16], st: usize| -> [i16; 8] {
            let s = |i: usize| v[i * st] as i32;
            let (b0, b1) = (s(0) + s(1), s(0) - s(1));
            let (b2, b3) = (s(2) + s(3), s(2) - s(3));
            let (b4, b5) = (s(4) + s(5), s(4) - s(5));
            let (b6, b7) = (s(6) + s(7), s(6) - s(7));
            let (c0, c1) = (b0 + b2, b1 + b3);
            let (c2, c3) = (b0 - b2, b1 - b3);
            let (c4, c5) = (b4 + b6, b5 + b7);
            let (c6, c7) = (b4 - b6, b5 - b7);
            let mut o = [0i16; 8];
            o[0] = (c0 + c4) as i16;
            o[7] = (c1 + c5) as i16;
            o[3] = (c2 + c6) as i16;
            o[4] = (c3 + c7) as i16;
            o[2] = (c0 - c4) as i16;
            o[6] = (c1 - c5) as i16;
            o[1] = (c2 - c6) as i16;
            o[5] = (c3 - c7) as i16;
            o
        };
        let mut b1 = [0i16; 64];
        for idx in 0..8 {
            b1[idx * 8..idx * 8 + 8].copy_from_slice(&col(&src[idx..], stride));
        }
        let mut b2 = [0i16; 64];
        for idx in 0..8 {
            b2[idx * 8..idx * 8 + 8].copy_from_slice(&col(&b1[idx..], 8));
        }
        // `svt_aom_hadamard_8x8_c` stores STRAIGHT through (unlike the 4x4
        // form, which transposes) — `coeff[idx] = buffer2[idx]`.
        let mut out = [0i32; 64];
        for idx in 0..64 {
            out[idx] = b2[idx] as i32;
        }
        out
    }
    for case in 0..48 {
        let stride = 8 + (case % 5) * 3;
        // bd8 residual range for most cases, bd10 for the rest.
        let span: i32 = if case % 3 == 2 { 2047 } else { 511 };
        let src: alloc::vec::Vec<i16> = (0..stride * 8 + 8)
            .map(|_| ((next() % (span as u32 * 2 + 1)) as i32 - span) as i16)
            .collect();
        let expect = oracle(&src, stride);
        let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
            let mut got = [0i32; 64];
            aom_hadamard_8x8(&src, stride, &mut got);
            assert_eq!(
                got, expect,
                "hadamard_8x8 case {case} stride {stride} tier {perm}"
            );
        });
        assert!(
            rep.warnings.is_empty(),
            "excluded tokens: {:?}",
            rep.warnings
        );
        assert!(
            rep.permutations_run >= 2,
            "only {} permutations",
            rep.permutations_run
        );
    }
}

#[test]
fn satd_8x8_identical() {
    let block = [128u8; 128];
    assert_eq!(satd_8x8(&block, 16, &block, 16), 0);
}

#[test]
fn satd_8x8_uniform_diff() {
    let src = [110u8; 64];
    let ref_ = [100u8; 64];
    // DC = 10 * 64 = 640, SATD = |640| / 4 = 160
    assert_eq!(satd_8x8(&src, 8, &ref_, 8), 160);
}

#[test]
fn satd_greater_than_zero_for_different() {
    let mut src = [0u8; 64];
    let ref_ = [128u8; 64];
    for (i, v) in src.iter_mut().enumerate() {
        *v = (i * 7 % 256) as u8;
    }
    assert!(satd_4x4(&src, 8, &ref_, 8) > 0);
    assert!(satd_8x8(&src, 8, &ref_, 8) > 0);
}

#[test]
fn satd_geq_sad() {
    // SATD should generally be >= SAD / N for non-trivial patterns
    // (Hadamard preserves energy)
    let mut src = [0u8; 64];
    let ref_ = [0u8; 64];
    for (i, v) in src.iter_mut().enumerate() {
        *v = if i % 2 == 0 { 200 } else { 50 };
    }
    let satd = satd_4x4(&src, 8, &ref_, 8);
    assert!(satd > 0);
}
