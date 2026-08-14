//! Variance and SSE (Sum of Squared Errors) computation.
//!
//! Spec 13 (segmentation.md): Variance for adaptive quantization.
//!
//! Variance is used for adaptive quantization, activity masking,
//! and screen content detection. SSE is the primary distortion metric
//! for rate-distortion optimization.

use archmage::prelude::*;

/// Compute variance of an 8-bit pixel block.
///
/// Returns (variance, mean) where variance = E[x²] - E[x]² scaled by N.
/// More precisely: variance = sum((x - mean)²) = sum(x²) - sum(x)²/N
pub fn variance(src: &[u8], src_stride: usize, width: usize, height: usize) -> (u64, u32) {
    incant!(
        variance_impl(src, src_stride, width, height),
        [v3, neon, scalar]
    )
}

/// C `svt_aom_variance{W}x{H}_c` — the TWO-BUFFER difference variance.
///
/// `variance_c` (`Lib/C_DEFAULT/variance.c:141`) accumulates the signed
/// difference `sum` and the squared difference `sse` over the block; the `VAR`
/// macro (`:184`) then returns
/// `*sse - (uint32_t)(((int64_t)sum * sum) / (W * H))`.
///
/// Distinct from [`variance`] above, which is the SINGLE-buffer activity
/// variance used by segmentation/AQ. This one is C's `AomVarianceFnPtr::vf`
/// (`Lib/Codec/av1me.c:30`) — the MDS0 fast distortion on the arm C takes when
/// `mds0_use_hadamard_blk` is false (`product_coding_loop.c:1296-1302`).
///
/// Exactness notes, all binding for byte identity:
/// * `sum` is C's `int`; the widest block here (128x128) reaches |sum| <= 4.18e6
///   and sse <= 1.07e9, so i32/u32 do not overflow — the u64/i64 accumulators
///   below are a superset and truncate to the same values.
/// * the division is C integer division of a non-negative `sum * sum` by
///   `w * h`, i.e. truncation, and it happens BEFORE the subtraction.
/// * the subtraction is in `uint32_t`. `sse >= sum^2/n` always (Cauchy-Schwarz),
///   so it cannot wrap; `wrapping_sub` is used to make that explicit rather than
///   to permit it.
pub fn variance_diff(
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> u32 {
    let (sse, sum) = incant!(
        variance_diff_parts_impl(a, a_stride, b, b_stride, width, height),
        [v3, neon, scalar]
    );
    let n = (width * height) as i64;
    (sse as u32).wrapping_sub(((sum * sum) / n) as u32)
}

/// Compute SSE between two blocks of 8-bit pixels.
pub fn sse(
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    incant!(
        sse_impl(src, src_stride, ref_, ref_stride, width, height),
        [v3, neon, scalar]
    )
}

// --- Scalar implementations ---

fn variance_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

/// `(sse, sum)` for [`variance_diff`] — scalar reference, mirrors C's
/// `variance_c` loop exactly.
fn variance_diff_parts_impl_scalar(
    _token: ScalarToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> (u64, i64) {
    let mut sse: u64 = 0;
    let mut sum: i64 = 0;
    for row in 0..height {
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        for col in 0..width {
            let diff = a[a_off + col] as i32 - b[b_off + col] as i32;
            sum += diff as i64;
            sse += (diff * diff) as u64;
        }
    }
    (sse, sum)
}

fn sse_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

// --- AVX2 implementations ---

#[cfg(target_arch = "x86_64")]
#[arcane]
fn variance_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    // Auto-vectorize with AVX2 enabled — compiler does well here
    let mut sum: u64 = 0;
    let mut sum_sq: u64 = 0;
    for row in 0..height {
        let offset = row * src_stride;
        for col in 0..width {
            let v = src[offset + col] as u64;
            sum += v;
            sum_sq += v * v;
        }
    }
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn variance_diff_parts_impl_v3(
    _token: Desktop64,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> (u64, i64) {
    // Auto-vectorizes well with AVX2 enabled; the shape is identical to the
    // scalar reference so the two cannot drift.
    let mut sse: u64 = 0;
    let mut sum: i64 = 0;
    for row in 0..height {
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        for col in 0..width {
            let diff = a[a_off + col] as i32 - b[b_off + col] as i32;
            sum += diff as i64;
            sse += (diff * diff) as u64;
        }
    }
    (sse, sum)
}

#[cfg(target_arch = "x86_64")]
#[arcane]
fn sse_impl_v3(
    _token: Desktop64,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    let mut sse: u64 = 0;
    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        for col in 0..width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            sse += (diff * diff) as u64;
        }
    }
    sse
}

// --- NEON implementations ---

#[cfg(target_arch = "aarch64")]
#[arcane]
fn variance_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    width: usize,
    height: usize,
) -> (u64, u32) {
    // Sum via the vpaddlq/vpadalq widening chain; sum-of-squares via vmull_u8
    // (u8*u8 fits u16) drained into u32 lanes every iteration.
    //
    // Overflow: a u16 lane holds at most 255*255 = 65025, so squares must NOT
    // accumulate in u16. Draining to u32 per 16-byte chunk keeps the largest
    // block used here (128x128 = 16384 px, worst case 65025*16384 = 1.07e9)
    // inside u32's 4.29e9.
    let mut sum_acc = vdupq_n_u32(0);
    let mut sq_acc = vdupq_n_u32(0);
    let mut tail_sum: u64 = 0;
    let mut tail_sq: u64 = 0;

    for row in 0..height {
        let off = row * src_stride;
        let mut col = 0;
        while col + 16 <= width {
            let c: &[u8; 16] = src[off + col..off + col + 16].try_into().unwrap();
            let v = vld1q_u8(c);
            sum_acc = vpadalq_u16(sum_acc, vpaddlq_u8(v));
            let lo = vget_low_u8(v);
            let hi = vget_high_u8(v);
            sq_acc = vpadalq_u16(sq_acc, vmull_u8(lo, lo));
            sq_acc = vpadalq_u16(sq_acc, vmull_u8(hi, hi));
            col += 16;
        }
        while col < width {
            let v = src[off + col] as u64;
            tail_sum += v;
            tail_sq += v * v;
            col += 1;
        }
    }

    let sum = vaddvq_u32(sum_acc) as u64 + tail_sum;
    let sum_sq = vaddvq_u32(sq_acc) as u64 + tail_sq;
    let n = (width * height) as u64;
    let variance = sum_sq * n - sum * sum;
    let mean = (sum / n) as u32;
    (variance, mean)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn variance_diff_parts_impl_neon(
    _token: NeonToken,
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    width: usize,
    height: usize,
) -> (u64, i64) {
    // `sum` of signed differences is computed as sum(a) - sum(b) — exact, and it
    // avoids needing a signed 8-bit subtract. Each is a vpaddlq/vpadalq widening
    // chain drained to u64 per ROW, so an arbitrarily tall block cannot overflow
    // the u32 accumulator (worst row: 128 px * 255 = 32640).
    //
    // `sse` uses vabdq_u8 (|a-b| is exact for u8, and |d|^2 == d^2) squared with
    // vmull_u8 into u16 lanes; squares reach 65025 so they are drained to u32
    // every 16-byte chunk, and the u32 total is drained to u64 per row.
    let mut sse: u64 = 0;
    let mut sum_a: u64 = 0;
    let mut sum_b: u64 = 0;

    for row in 0..height {
        let a_off = row * a_stride;
        let b_off = row * b_stride;
        let mut col = 0;
        let mut sse_acc = vdupq_n_u32(0);
        let mut a_acc = vdupq_n_u32(0);
        let mut b_acc = vdupq_n_u32(0);
        while col + 16 <= width {
            let ac: &[u8; 16] = a[a_off + col..a_off + col + 16].try_into().unwrap();
            let bc: &[u8; 16] = b[b_off + col..b_off + col + 16].try_into().unwrap();
            let va = vld1q_u8(ac);
            let vb = vld1q_u8(bc);
            a_acc = vpadalq_u16(a_acc, vpaddlq_u8(va));
            b_acc = vpadalq_u16(b_acc, vpaddlq_u8(vb));
            let d = vabdq_u8(va, vb);
            let lo = vget_low_u8(d);
            let hi = vget_high_u8(d);
            sse_acc = vpadalq_u16(sse_acc, vmull_u8(lo, lo));
            sse_acc = vpadalq_u16(sse_acc, vmull_u8(hi, hi));
            col += 16;
        }
        sse += vaddvq_u32(sse_acc) as u64;
        sum_a += vaddvq_u32(a_acc) as u64;
        sum_b += vaddvq_u32(b_acc) as u64;
        while col < width {
            let av = a[a_off + col] as u64;
            let bv = b[b_off + col] as u64;
            let diff = av as i64 - bv as i64;
            sum_a += av;
            sum_b += bv;
            sse += (diff * diff) as u64;
            col += 1;
        }
    }

    (sse, sum_a as i64 - sum_b as i64)
}

#[cfg(target_arch = "aarch64")]
#[arcane]
fn sse_impl_neon(
    _token: NeonToken,
    src: &[u8],
    src_stride: usize,
    ref_: &[u8],
    ref_stride: usize,
    width: usize,
    height: usize,
) -> u64 {
    // |a-b| via vabdq_u8 (exact for u8), squared with vmull_u8 into u16 and
    // drained to u32 each chunk — squares reach 65025 so they must not
    // accumulate in u16. The u32 accumulator is drained to u64 per ROW, so an
    // arbitrarily tall block cannot overflow it.
    let mut total: u64 = 0;
    let mut tail: u64 = 0;

    for row in 0..height {
        let s_off = row * src_stride;
        let r_off = row * ref_stride;
        let mut col = 0;
        let mut acc = vdupq_n_u32(0);

        while col + 16 <= width {
            let a: &[u8; 16] = src[s_off + col..s_off + col + 16].try_into().unwrap();
            let b: &[u8; 16] = ref_[r_off + col..r_off + col + 16].try_into().unwrap();
            let d = vabdq_u8(vld1q_u8(a), vld1q_u8(b));
            let lo = vget_low_u8(d);
            let hi = vget_high_u8(d);
            acc = vpadalq_u16(acc, vmull_u8(lo, lo));
            acc = vpadalq_u16(acc, vmull_u8(hi, hi));
            col += 16;
        }
        total += vaddvq_u32(acc) as u64;

        while col < width {
            let diff = src[s_off + col] as i32 - ref_[r_off + col] as i32;
            tail += (diff * diff) as u64;
            col += 1;
        }
    }
    total + tail
}

#[cfg(test)]
mod tests {
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
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    use alloc::vec::Vec;
    use archmage::testing::{CompileTimePolicy, for_each_token_permutation};

    #[test]
    fn variance_all_dispatch_levels() {
        let block: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let reference_result = variance(&block, 8, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = variance(&block, 8, 8, 8);
            assert_eq!(
                result, reference_result,
                "variance mismatch at dispatch level"
            );
        });
    }

    #[test]
    fn sse_all_dispatch_levels() {
        let src: Vec<u8> = (0..64).map(|i| (i * 3 + 17) as u8).collect();
        let ref_: Vec<u8> = (0..64).map(|i| (i * 5 + 42) as u8).collect();
        let reference_result = sse(&src, 8, &ref_, 8, 8, 8);

        let _ = for_each_token_permutation(CompileTimePolicy::WarnStderr, |_perm| {
            let result = sse(&src, 8, &ref_, 8, 8, 8);
            assert_eq!(result, reference_result, "sse mismatch at dispatch level");
        });
    }
}
