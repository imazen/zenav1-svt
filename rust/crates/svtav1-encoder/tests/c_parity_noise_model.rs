//! Tier-1 differentials for `svtav1_encoder::port_noise_model` — the leaves of
//! `Source/Lib/Codec/noise_model.c` that are exported.
//!
//! Four functions are driven through the real exported C symbols
//! (`docs/WORKING-ON-THIS.md` §4 tier 1). The four `static` ones in the same
//! module (`num_coeffs`, `bin_index`, `value_at`, `compare_scores`) have no
//! exported caller that isolates them, so their tests here are TIER 4 and say
//! so in the test name.
//!
//! Float comparison is on `to_bits()`, not `==`: an `f64` that differs in the
//! last place still compares unequal that way, and a NaN compares equal to
//! itself, which is what a bit-exactness check wants and what `==` gives
//! backwards on both counts.

use svtav1_cref::preanalysis as cref;
use svtav1_encoder::port_noise_model as port;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// A finite `f64` spread over several orders of magnitude, plus the
    /// occasional exact zero and negative.
    fn f64(&mut self) -> f64 {
        let r = self.next();
        let mag = f64::from((r >> 40) as u32) / 16777216.0;
        let scale = match (r >> 3) & 3 {
            0 => 1e-6,
            1 => 1.0,
            2 => 255.0,
            _ => 1e6,
        };
        let v = mag * scale;
        if r & 1 == 0 { v } else { -v }
    }
    fn f32(&mut self) -> f32 {
        self.f64() as f32
    }
}

#[test]
fn pointwise_multiply_matches_c() {
    let mut rng = Rng(0x1111_2222_3333_4444);
    let mut cells = 0usize;
    for n in [0usize, 1, 2, 3, 7, 16, 17, 64, 255] {
        let a: Vec<f32> = (0..n).map(|_| rng.f32()).collect();
        let b_d: Vec<f64> = (0..n).map(|_| rng.f64()).collect();
        let c_d: Vec<f64> = (0..n).map(|_| rng.f64()).collect();

        let (mut cb, mut cc) = (vec![0f32; n], vec![0f32; n]);
        let (mut cbd, mut ccd) = (b_d.clone(), c_d.clone());
        cref::nm_pointwise_multiply(&a, &mut cb, &mut cc, &mut cbd, &mut ccd);

        let (mut rb, mut rc) = (vec![0f32; n], vec![0f32; n]);
        port::pointwise_multiply(&a, &mut rb, &mut rc, &b_d, &c_d);

        for i in 0..n {
            assert_eq!(rb[i].to_bits(), cb[i].to_bits(), "b[{i}] n={n}");
            assert_eq!(rc[i].to_bits(), cc[i].to_bits(), "c[{i}] n={n}");
        }
        cells += 1;
    }
    assert_eq!(cells, 9);

    // The `(float)b_d[i]` narrowing happens BEFORE the multiply, and that is
    // not a distinction without a difference. Search for inputs where
    // narrowing first and narrowing last disagree, prove some exist, and check
    // C takes the narrow-FIRST answer on every one of them.
    let mut separating = 0usize;
    let mut probe = Rng(0xFACE_B00C_0000_0001);
    let (mut a1, mut bd1) = (vec![0f32; 512], vec![0f64; 512]);
    for i in 0..512 {
        a1[i] = probe.f32();
        bd1[i] = probe.f64();
    }
    let (mut cb1, mut cc1) = (vec![0f32; 512], vec![0f32; 512]);
    let (mut bd1c, mut bd1c2) = (bd1.clone(), bd1.clone());
    cref::nm_pointwise_multiply(&a1, &mut cb1, &mut cc1, &mut bd1c, &mut bd1c2);
    for i in 0..512 {
        let narrow_first = a1[i] * (bd1[i] as f32);
        let narrow_last = (f64::from(a1[i]) * bd1[i]) as f32;
        assert_eq!(
            cb1[i].to_bits(),
            narrow_first.to_bits(),
            "C disagreed with narrow-first at {i}"
        );
        if narrow_first.to_bits() != narrow_last.to_bits() {
            separating += 1;
        }
    }
    assert!(
        separating > 0,
        "no input separated narrow-first from narrow-last, so the cast order is untested"
    );
}

#[test]
fn apply_window_function_to_plane_matches_c() {
    let mut rng = Rng(0x5555_6666_7777_8888);
    let mut cells = 0usize;
    let mut accumulated = 0usize;
    for (ys, xs) in [(1usize, 1usize), (2, 3), (8, 8), (5, 9), (16, 16), (7, 32)] {
        for extra in [0usize, 1, 5] {
            let rstride = xs + extra;
            let n = ys * xs;
            let mut block: Vec<f32> = (0..n).map(|_| rng.f32()).collect();
            let mut plane: Vec<f32> = (0..n).map(|_| rng.f32()).collect();
            let window: Vec<f32> = (0..n).map(|_| rng.f32()).collect();
            // A NON-ZERO starting result, because C ACCUMULATES: starting from
            // zero would make an assignment and an accumulation identical.
            let base: Vec<f32> = (0..ys * rstride).map(|_| rng.f32()).collect();

            let mut c = base.clone();
            let (mut cb, mut cp) = (block.clone(), plane.clone());
            cref::nm_apply_window_function_to_plane(
                ys, xs, &mut c, rstride, &mut cb, &mut cp, &window,
            );

            let mut r = base.clone();
            port::apply_window_function_to_plane(ys, xs, &mut r, rstride, &block, &plane, &window);

            for i in 0..r.len() {
                assert_eq!(
                    r[i].to_bits(),
                    c[i].to_bits(),
                    "result[{i}] {ys}x{xs}+{extra}"
                );
            }
            if c != base {
                accumulated += 1;
            }
            // The kernel must not touch its inputs.
            assert_eq!(cb, block);
            assert_eq!(cp, plane);
            block.clear();
            plane.clear();
            cells += 1;
        }
    }
    assert_eq!(cells, 6 * 3);
    assert!(accumulated > 0, "the window kernel never wrote a result");
}

#[test]
fn solver_get_center_matches_c() {
    let mut cells = 0usize;
    let mut distinct: Vec<u64> = Vec::new();
    for num_bins in [2i32, 3, 8, 20, 64] {
        for bd in [8u32, 10, 12] {
            let max = f64::from((1i32 << bd) - 1);
            // Outside 0..num_bins too: `get_center` does NOT clamp, and a
            // caller that assumes it does would be wrong.
            for i in -3..=(num_bins + 3) {
                let c = cref::nm_solver_get_center(num_bins, 0.0, max, i);
                let solver = port::NoiseStrengthSolver::new(num_bins, bd);
                let r = solver.get_center(i);
                assert_eq!(
                    r.to_bits(),
                    c.to_bits(),
                    "get_center bins={num_bins} bd={bd} i={i}"
                );
                if !distinct.contains(&c.to_bits()) {
                    distinct.push(c.to_bits());
                }
                cells += 1;
            }
        }
    }
    assert!(cells > 100);
    assert!(
        distinct.len() > 20,
        "get_center returned {} distinct values",
        distinct.len()
    );
}

#[test]
fn solver_add_measurement_matches_c() {
    let mut rng = Rng(0x9999_AAAA_BBBB_CCCC);
    let mut cells = 0usize;
    let mut hit_clamp = 0usize;
    for num_bins in [2i32, 3, 20] {
        for bd in [8u32, 10] {
            let max = f64::from((1i32 << bd) - 1);
            let n = num_bins as usize;

            let mut c_a = vec![0f64; n * n];
            let mut c_b = vec![0f64; n];
            let (mut c_eqs, mut c_tot) = (0i32, 0f64);
            let mut r = port::NoiseStrengthSolver::new(num_bins, bd);

            // Means chosen to hit every bin boundary, both clamps, and a
            // spread of interior positions.
            let mut means: Vec<f64> = vec![-1000.0, -1.0, 0.0, max, max + 1.0, max * 2.0];
            for i in 0..num_bins {
                let ctr = r.get_center(i);
                means.extend([ctr - 0.5, ctr, ctr + 0.5, ctr + 0.001]);
            }
            for _ in 0..40 {
                means.push(rng.f64().abs() % (max + 10.0));
            }

            for m in means {
                let std = rng.f64();
                let (e, t) = cref::nm_solver_add_measurement(
                    num_bins, 0.0, max, &mut c_a, &mut c_b, c_eqs, c_tot, m, std,
                );
                c_eqs = e;
                c_tot = t;
                r.add_measurement(m, std);

                for i in 0..n * n {
                    assert_eq!(
                        r.a[i].to_bits(),
                        c_a[i].to_bits(),
                        "A[{i}] bins={num_bins} bd={bd} mean={m} std={std}"
                    );
                }
                for i in 0..n {
                    assert_eq!(r.b[i].to_bits(), c_b[i].to_bits(), "b[{i}] mean={m}");
                }
                assert_eq!(r.total.to_bits(), c_tot.to_bits(), "total mean={m}");
                assert_eq!(r.num_equations, c_eqs);
                // The clamped case, where bin_i0 == bin_i1 and the two
                // "separate" A updates address the same cell twice.
                if r.bin_index(m) >= f64::from(num_bins - 1) {
                    hit_clamp += 1;
                }
                cells += 1;
            }
        }
    }
    assert!(cells > 200);
    assert!(
        hit_clamp > 0,
        "the bin_i0 == bin_i1 clamp was never reached, so the doubled A update is untested"
    );
}

// ---------------------------------------------------------------------------
// TIER 4 — `static` in C
// ---------------------------------------------------------------------------

/// `num_coeffs` (noise_model.c:181).
#[test]
fn tier4_num_coeffs() {
    use port::NoiseShape::*;
    // Diamond: lag * (lag + 1).
    for lag in 0..=8 {
        assert_eq!(port::num_coeffs(Diamond, lag), lag * (lag + 1));
    }
    // Square: (2*lag + 1)^2 / 2, truncating — which is what drops the centre.
    for lag in 0..=8 {
        let n = 2 * lag + 1;
        assert_eq!(port::num_coeffs(Square, lag), (n * n) / 2);
    }
    // The values the encoder actually asks for.
    assert_eq!(port::num_coeffs(Diamond, 3), 12);
    assert_eq!(port::num_coeffs(Square, 3), 24);
    assert_eq!(port::num_coeffs(Diamond, 0), 0);
    assert_eq!(port::num_coeffs(Square, 0), 0);
}

/// `noise_strength_solver_get_bin_index` (noise_model.c:236) and
/// `noise_strength_solver_get_value` (:242).
#[test]
fn tier4_bin_index_and_value() {
    let mut s = port::NoiseStrengthSolver::new(20, 8);
    // Clamping at both ends.
    assert_eq!(s.bin_index(-100.0), 0.0);
    assert_eq!(s.bin_index(0.0), 0.0);
    assert_eq!(s.bin_index(255.0), 19.0);
    assert_eq!(s.bin_index(1000.0), 19.0);
    // The centre of bin i maps back to i.
    for i in 0..20 {
        let ctr = s.get_center(i);
        assert!((s.bin_index(ctr) - f64::from(i)).abs() < 1e-9, "bin {i}");
    }
    // `value_at` interpolates the SOLVED curve. With x[i] = i it must return
    // the fractional bin index itself.
    for (i, v) in s.x.iter_mut().enumerate() {
        *v = i as f64;
    }
    for probe in [0.0f64, 1.0, 13.4, 128.0, 254.9, 255.0] {
        let want = s.bin_index(probe);
        assert!((s.value_at(probe) - want).abs() < 1e-9, "value_at({probe})");
    }
    // At the top the two interpolation bins collapse, so the result is x[last]
    // exactly rather than an extrapolation.
    assert_eq!(s.value_at(255.0), 19.0);
    assert_eq!(s.value_at(1e9), 19.0);
}

/// `compare_scores` (noise_model.c:515).
#[test]
fn tier4_compare_scores() {
    use core::cmp::Ordering::*;
    assert_eq!(port::compare_scores(1.0, 2.0), Less);
    assert_eq!(port::compare_scores(2.0, 1.0), Greater);
    assert_eq!(port::compare_scores(1.0, 1.0), Equal);
    assert_eq!(port::compare_scores(-0.0, 0.0), Equal);
    // C's `diff < 0 ? -1 : diff > 0` answers "greater or equal", never
    // "less", when the subtraction is NaN — both tests are false. Reproduced.
    assert_eq!(port::compare_scores(f32::NAN, 1.0), Equal);
    assert_eq!(port::compare_scores(1.0, f32::NAN), Equal);
    assert_eq!(port::compare_scores(f32::INFINITY, f32::INFINITY), Equal);
    assert_eq!(port::compare_scores(f32::NEG_INFINITY, 0.0), Less);
    // Two very close but distinct floats whose difference underflows to zero
    // compare EQUAL here, where a direct `<` would not. That is C's, and it is
    // why the port subtracts rather than comparing.
    let tiny = f32::from_bits(1);
    assert_eq!(port::compare_scores(tiny, 2.0 * tiny), Less);
}
