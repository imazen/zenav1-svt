use super::*;
use alloc::vec;
use alloc::vec::Vec;
use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// Every dispatch tier reproduces the scalar core bit for bit, across every
/// AV1 transform width/height AND widths that are not multiples of the
/// vector body (so the scalar tail runs), including the saturating extremes
/// of the recon clamp.
#[test]
fn residual_recon_distortion_all_tiers_match_core() {
    let mut st = 0x5EED_1234_u64;
    for &(w, h) in &[
        (4usize, 4usize),
        (8, 8),
        (16, 16),
        (32, 32),
        (64, 64),
        (4, 16),
        (16, 4),
        (8, 32),
        (32, 8),
        (5, 3),
        (7, 9),
        (13, 2),
        (1, 1),
        (3, 1),
    ] {
        let sstride = w + 7;
        let pstride = w + 3;
        let src: Vec<u8> = (0..sstride * h + 32)
            .map(|_| (lcg(&mut st) & 0xff) as u8)
            .collect();
        let pred: Vec<u8> = (0..pstride * h + 32)
            .map(|_| (lcg(&mut st) & 0xff) as u8)
            .collect();
        // Residual
        let mut want = vec![0i32; w * h];
        residual_i32_core(&src, sstride, &pred, pstride, w, h, &mut want);
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
            let mut got = vec![0i32; w * h];
            residual_i32(&src, sstride, &pred, pstride, w, h, &mut got);
            assert_eq!(got, want, "residual {w}x{h}");
        });
        // i16 twin, same grid and the same tier sweep.
        let mut want16 = vec![0i16; w * h];
        residual_i16_core(&src, sstride, &pred, pstride, w, h, &mut want16);
        assert!(
            want16.iter().zip(&want).all(|(&a, &b)| i32::from(a) == b),
            "residual_i16 core disagrees with residual_i32 core at {w}x{h}"
        );
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
            let mut got = vec![0i16; w * h];
            residual_i16(&src, sstride, &pred, pstride, w, h, &mut got);
            assert_eq!(got, want16, "residual_i16 {w}x{h}");
        });
        // Recon: inv values chosen to straddle both clamp bounds hard.
        let inv: Vec<i32> = (0..w * h)
            .map(|i| match i % 5 {
                0 => -100000,
                1 => -255,
                2 => 0,
                3 => 255,
                _ => 100000,
            })
            .collect();
        let mut rwant = vec![0u8; w * h];
        recon_add_clamp_core(&pred, pstride, &inv, w, h, &mut rwant);
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
            let mut got = vec![0u8; w * h];
            recon_add_clamp(&pred, pstride, &inv, w, h, &mut got);
            assert_eq!(got, rwant, "recon {w}x{h}");
        });
        // Coefficient-domain distortion, including large magnitudes.
        let ca: Vec<i32> = (0..w * h)
            .map(|_| (lcg(&mut st) as i32) >> ((lcg(&mut st) % 20) as u32))
            .collect();
        let cb: Vec<i32> = (0..w * h)
            .map(|_| (lcg(&mut st) as i32) >> (12 + (lcg(&mut st) % 8) as u32))
            .collect();
        let dwant = sse_i32_core(&ca, &cb);
        let swant = sq_sum_i32_core(&ca);
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
            assert_eq!(sse_i32(&ca, &cb), dwant, "sse_i32 {w}x{h}");
            assert_eq!(sq_sum_i32(&ca), swant, "sq_sum_i32 {w}x{h}");
        });
    }
}

/// C's residual term computed in `i128` and reduced mod 2^64 at the very
/// end. It shares no code with the kernels under test: the arithmetic is
/// exact and only the final reduction reproduces `uint64_t`'s wrap, so it
/// pins BOTH of C's widths (`pic_operators.c:86`) independently of how the
/// port spells them.
fn sse_oracle_i128(a: &[i32], b: &[i32]) -> u64 {
    let mut acc: i128 = 0;
    for (&x, &y) in a.iter().zip(b) {
        let e = x as i128 - y as i128;
        acc += e * e;
    }
    (acc as u128 & u64::MAX as u128) as u64
}

/// The width bug this test exists to catch: subtract in i32 (wrapping),
/// THEN widen. Present only so the test can PROVE its inputs discriminate
/// the two forms — a case set where this agrees with the oracle would make
/// the assertions below vacuous.
fn sse_i32_subtract_then_widen(a: &[i32], b: &[i32]) -> u64 {
    let mut d: u64 = 0;
    for (&x, &y) in a.iter().zip(b) {
        let e = x.wrapping_sub(y) as i64;
        d = d.wrapping_add(e.wrapping_mul(e) as u64);
    }
    d
}

/// Every tier reproduces C's `int64_t` subtraction and wrapping `uint64_t`
/// accumulator at inputs where a 32-bit subtraction would wrap.
///
/// Measured 2026-08-11: this does NOT happen on a real encode (0 wraps in
/// 59,088,480 elements; max |difference| 788 against an i32 ceiling of
/// 2,147,483,647 — `benchmarks/sse_i32_width_2026-08-11.meta`). The gate
/// is on the arithmetic contract, so that a future caller with a wider
/// coefficient domain — 10/12-bit, a lossless path, an inter residual —
/// cannot silently inherit a wrapped distortion.
#[test]
fn sse_i32_matches_c_widths_at_i32_extremes() {
    // Cases chosen so the wrap lands in the VECTOR body, in the SCALAR
    // tail, and in a length below one vector — the three places the NEON
    // arm treats differently.
    let cases: [(alloc::vec::Vec<i32>, alloc::vec::Vec<i32>); 6] = [
        // Widest possible difference (2^32 - 1); its square exceeds i64.
        (vec![i32::MAX; 8], vec![i32::MIN; 8]),
        (vec![i32::MIN; 8], vec![i32::MAX; 8]),
        // Wrap only in the 3-element tail of a 7-element slice. (The two
        // wrapped terms must not be symmetric: with `i32::MAX` in both
        // slots their mod-2^64 errors are +-2^33 and CANCEL, and the case
        // silently stops discriminating.)
        (
            vec![1, 2, 3, 4, i32::MAX, i32::MAX - 5, 6],
            vec![1, 0, 3, 0, -2, i32::MIN, 6],
        ),
        // Wrap only in the vector body; clean tail.
        (
            vec![i32::MIN, 5, -7, 9, 11, 13, 15],
            vec![i32::MAX, 1, 1, 1, 1, 1, 1],
        ),
        // Shorter than one vector.
        (vec![i32::MIN, 3], vec![7, 3]),
        // Accumulator wrap: 64 squares of ~2^62 sum past 2^64.
        (vec![i32::MIN; 64], vec![0; 64]),
    ];
    let mut discriminating = 0usize;
    for (i, (a, b)) in cases.iter().enumerate() {
        let want = sse_oracle_i128(a, b);
        if sse_i32_subtract_then_widen(a, b) != want {
            discriminating += 1;
        }
        assert_eq!(sse_i32_core(a, b), want, "scalar core, case {i}");
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
            assert_eq!(sse_i32(a, b), want, "sse_i32 case {i}");
        });
        // sq_sum's own accumulator must wrap like C's uint64_t too.
        let sq_want = {
            let mut acc: i128 = 0;
            for &x in a {
                acc += (x as i128) * (x as i128);
            }
            (acc as u128 & u64::MAX as u128) as u64
        };
        assert_eq!(sq_sum_i32_core(a), sq_want, "sq_sum core, case {i}");
        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_p| {
            assert_eq!(sq_sum_i32(a), sq_want, "sq_sum_i32 case {i}");
        });
    }
    // Anti-vacuity: without this the case set could be all small values and
    // every assertion above would pass on the broken kernel too.
    // Case 5 carries no wrap by construction (it exists for the
    // accumulator), so 5 of 6 is the maximum available here.
    assert_eq!(
        discriminating, 5,
        "expected 5 of 6 cases to distinguish an i32 subtraction from C's \
             i64 one — the gate has no teeth"
    );
}
