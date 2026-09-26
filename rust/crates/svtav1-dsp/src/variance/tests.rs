use super::*;

#[test]
fn variance_uniform_block() {
    let block = [128u8; 64];
    let (var, mean) = variance(&block, 8, 8, 8);
    assert_eq!(var, 0, "uniform block should have zero variance");
    assert_eq!(mean, 128);
}

#[test]
fn variance_known_values() {
    // 4x4 block: 0,1,2,...,15
    let mut block = [0u8; 16];
    for (i, b) in block.iter_mut().enumerate() {
        *b = i as u8;
    }
    let (var, _mean) = variance(&block, 4, 4, 4);
    // sum = 120, sum_sq = 1240, n = 16
    // var = 1240 * 16 - 120 * 120 = 19840 - 14400 = 5440
    assert_eq!(var, 5440);
}

#[test]
fn sse_identical_blocks() {
    let block = [42u8; 64];
    assert_eq!(sse(&block, 8, &block, 8, 8, 8), 0);
}

/// C `variance_c` + `VAR(W,H)` by hand on a case where the mean difference
/// is NON-ZERO, so the `- sum*sum/n` term is load-bearing (an implementation
/// that returned plain SSE would pass an identical-mean test).
#[test]
fn variance_diff_known_values() {
    // 4x4: a = 0..15, b = all 4. diffs -4..11.
    let mut a = [0u8; 16];
    for (i, v) in a.iter_mut().enumerate() {
        *v = i as u8;
    }
    let b = [4u8; 16];
    // sum  = (0+..+15) - 16*4 = 120 - 64 = 56
    // sse  = sum over d in -4..=11 of d*d = (16+9+4+1) + 11*12*23/6 = 536
    // var  = 536 - (56*56)/16 = 536 - 196 = 340
    assert_eq!(variance_diff(&a, 4, &b, 4, 4, 4), 340);
    // identical blocks: sum = 0, sse = 0.
    assert_eq!(variance_diff(&a, 4, &a, 4, 4, 4), 0);
}

/// Every dispatch tier must agree with the scalar core on random content,
/// at strides wider than the block (the MDS0 caller passes a full-frame
/// source stride against a tightly-packed prediction) and at widths that
/// are not a multiple of the 16-lane NEON chunk, which is where a tail bug
/// hides. Consumes the `PermutationReport` — a bare call would silently
/// degrade to native-tier-only coverage (see rust/CLAUDE.md, Archmage Rules).
#[test]
fn variance_diff_random_all_tiers_match_scalar() {
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};
    let mut st = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u8
    };
    // 4x4 .. 64x64 square + the non-square and non-16-multiple shapes.
    for &(w, h) in &[
        (4usize, 4usize),
        (4, 8),
        (8, 4),
        (8, 8),
        (12, 4),
        (16, 16),
        (16, 4),
        (4, 16),
        (20, 12),
        (32, 32),
        (32, 8),
        (64, 64),
        (64, 16),
        (128, 16),
        (96, 8),
    ] {
        for pad in [0usize, 7] {
            let (astr, bstr) = (w + pad, w + 2 * pad);
            let a: alloc::vec::Vec<u8> = (0..astr * h).map(|_| next()).collect();
            let b: alloc::vec::Vec<u8> = (0..bstr * h).map(|_| next()).collect();
            // Independent oracle, written straight from C's variance_c +
            // VAR(W,H) rather than by calling the scalar arm — so a bug
            // shared by every arm still fails the test.
            let (mut sse_ref, mut sum_ref) = (0i64, 0i64);
            for r in 0..h {
                for c in 0..w {
                    let d = a[r * astr + c] as i64 - b[r * bstr + c] as i64;
                    sum_ref += d;
                    sse_ref += d * d;
                }
            }
            let n = (w * h) as i64;
            let expect = (sse_ref as u32).wrapping_sub(((sum_ref * sum_ref) / n) as u32);
            let rep = for_each_token_permutation(CompileTimePolicy::WarnStderr, |perm| {
                assert_eq!(
                    variance_diff(&a, astr, &b, bstr, w, h),
                    expect,
                    "variance_diff {w}x{h} strides ({astr},{bstr}) tier {perm}"
                );
            });
            assert!(
                rep.warnings.is_empty(),
                "tokens excluded at compile time, coverage is not what it looks like: {:?}",
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

/// Positive witness: on an AVX-512 host the `_v4` arm itself must be
/// exercised — the permutation test alone cannot distinguish "v4 ran"
/// from a silent fall-through to v3. Covers every `pack_rows_v4` width,
/// the >=64 chunked path, its staged remainder, and the odd-width
/// staged path.
#[test]
fn variance_diff_v4_arm_matches_scalar_when_summoned() {
    extern crate std;
    let Some(v4) = X64V4Token::summon() else {
        std::eprintln!("X64V4Token unavailable — _v4 arm NOT exercised on this host");
        return;
    };
    let mut st = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        (st >> 33) as u8
    };
    for &(w, h) in &[
        (4usize, 20usize),
        (8, 12),
        (16, 20),
        (32, 6),
        (64, 8),
        (128, 8),
        (96, 8),
        (12, 4),
        (24, 8),
    ] {
        for pad in [0usize, 5] {
            let (astr, bstr) = (w + pad, w + 2 * pad);
            let a: alloc::vec::Vec<u8> = (0..astr * h).map(|_| next()).collect();
            let b: alloc::vec::Vec<u8> = (0..bstr * h).map(|_| next()).collect();
            let want = variance_diff_parts_impl_scalar(
                ScalarToken::summon().unwrap(),
                &a,
                astr,
                &b,
                bstr,
                w,
                h,
            );
            assert_eq!(
                variance_diff_parts_impl_v4(v4, &a, astr, &b, bstr, w, h),
                want,
                "variance_diff_parts_impl_v4 {w}x{h} strides ({astr},{bstr})"
            );
        }
    }
}

#[test]
fn sse_known_value() {
    let src = [10u8; 16];
    let ref_ = [20u8; 16];
    // Each pixel diff = 10, diff² = 100, 16 pixels => SSE = 1600
    assert_eq!(sse(&src, 4, &ref_, 4, 4, 4), 1600);
}

#[test]
fn sse_max_difference() {
    let src = [0u8; 16];
    let ref_ = [255u8; 16];
    assert_eq!(sse(&src, 4, &ref_, 4, 4, 4), 255 * 255 * 16);
}
