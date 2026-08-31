//! RANSAC global-motion model fitting — a port of `Codec/ransac.c`, plus the
//! `random.h` / `mathutils.h` helpers it inlines.
//!
//! # Why it is here
//!
//! `determine_gm_params` (`global_motion.c:364`) is a one-line wrapper over
//! `svt_aom_ransac`. The triage that scoped this lane said, correctly, that
//! "the real cost of that item is ransac.c, not the wrapper". This is that
//! cost paid: 434 lines of double-precision least squares plus a PRNG-driven
//! sample draw. `Codec/ransac.c` is a different module group; it is ported
//! here because it is the only thing between the ported GM chain and a
//! callable `determine_gm_params`, and because `svt_aom_ransac` is EXPORTED so
//! it lands at tier 1 rather than as an unverified stub.
//!
//! # Determinism notes — read before trusting cross-host results
//!
//! * **The model comparison can TIE, and C breaks ties with `qsort`, which is
//!   not stable and whose permutation for equal elements is
//!   implementation-defined.** `compare_motions` returns 0 when two motions
//!   have the same `num_inliers` AND the same `sse`, so on a tie glibc's
//!   `qsort` and macOS libc's `qsort` may order them differently — C is not
//!   self-consistent across libcs there, let alone against a port. This port
//!   uses a STABLE sort. In practice the only reachable ties are between
//!   entries that were never filled (both `(0, 0.0)`), and those all emit the
//!   identity model and are skipped, so the observable output is unaffected;
//!   two genuinely-distinct fitted models tying to the last bit of an `f64`
//!   `sse` is not a case anyone has produced. Recorded so nobody discovers it
//!   as a mystery cross-host divergence.
//! * **The PRNG is seeded from `npoints`**, so the whole search is a pure
//!   function of the correspondence set — there is no wall-clock or address
//!   entropy to chase.
//! * **All arithmetic is `f64`** and is accumulated in C's exact order.
//!   `f64` addition is not associative, so reordering any of these sums is a
//!   different number.
//!
//! # Evidence
//!
//! Tier 1 — `tests/c_parity_ransac.rs` drives the real exported
//! `svt_aom_ransac`. Everything else in `ransac.c` is `static` and is covered
//! transitively through it, which is exactly what drives them in C.

use svtav1_types::motion::MAX_PARAMDIM;

/// `Correspondence` (global_motion.h:26) — a matched point pair in the frame
/// (`x`, `y`) and the reference (`rx`, `ry`). Integer coordinates, widened to
/// `f64` inside the scorers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Correspondence {
    pub x: i32,
    pub y: i32,
    pub rx: i32,
    pub ry: i32,
}

/// `MotionModel` (global_motion.h:38) — one candidate model plus the inlier
/// points that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionModel {
    pub params: [f64; MAX_PARAMDIM],
    /// Interleaved `[x0, y0, x1, y1, ...]`, `num_inliers` pairs.
    pub inliers: alloc::vec::Vec<i32>,
    pub num_inliers: usize,
}

impl Default for MotionModel {
    fn default() -> Self {
        Self {
            params: IDENTITY_PARAMS,
            inliers: alloc::vec::Vec::new(),
            num_inliers: 0,
        }
    }
}

/// `kIdentityParams` (ransac.h:24).
pub const IDENTITY_PARAMS: [f64; MAX_PARAMDIM] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
/// `MIN_INLIER_PROB` (ransac.h:22).
pub const MIN_INLIER_PROB: f64 = 0.1;
/// `MAX_MINPTS` (ransac.c:22).
pub const MAX_MINPTS: usize = 4;
/// `MINPTS_MULTIPLIER` (ransac.c:24).
pub const MINPTS_MULTIPLIER: usize = 5;
/// `INLIER_THRESHOLD_SQUARED` (ransac.c:26) — `1.25 * 1.25`.
pub const INLIER_THRESHOLD_SQUARED: f64 = 1.5625;
/// `NUM_TRIALS` (ransac.c:29).
pub const NUM_TRIALS: usize = 20;
/// `NUM_REFINES` (ransac.c:32).
pub const NUM_REFINES: usize = 5;

// --------------------------------------------------------------------------
// random.h
// --------------------------------------------------------------------------

/// Port of `lcg_next` (random.h:23). The multiply is done in 64 bits and then
/// TRUNCATED to 32 — doing it in 32 bits directly is the same modulo 2^32, but
/// the port keeps C's shape so the wrap is explicit rather than accidental.
#[inline]
pub fn lcg_next(state: &mut u32) -> u32 {
    *state = ((u64::from(*state) * 1_103_515_245u64).wrapping_add(12345)) as u32;
    *state
}

/// Port of `lcg_randint` (random.h:38) — `(next * n) >> 32`, which uses the
/// HIGH bits of the generator. C's own comment explains why: the low bits of
/// this LCG are comparatively poor, so `rand() % n` would be biased.
#[inline]
pub fn lcg_randint(state: &mut u32, n: u32) -> u32 {
    ((u64::from(lcg_next(state)) * u64::from(n)) >> 32) as u32
}

/// Port of `lcg_pick` (random.h:55) — pick `k` DISTINCT values from
/// `0..n`, by resampling on a repeat.
///
/// The resampling loop is C's `goto resample`, which restarts the draw for
/// index `i` only; the already-accepted values are kept. With `n >> k` this
/// terminates quickly, and every call site guarantees
/// `npoints >= minpts * MINPTS_MULTIPLIER`.
pub fn lcg_pick(n: usize, k: usize, out: &mut [i32], seed: &mut u32) {
    debug_assert!(k <= n);
    for i in 0..k {
        loop {
            let v = lcg_randint(seed, n as u32) as i32;
            if out[..i].contains(&v) {
                continue;
            }
            out[i] = v;
            break;
        }
    }
}

// --------------------------------------------------------------------------
// mathutils.h
// --------------------------------------------------------------------------

/// `tiny_near_zero` (mathutils.h:23).
const TINY_NEAR_ZERO: f64 = 1.0e-16;

/// Port of `linsolve` (mathutils.h:22) — Gaussian elimination with a partial
/// pivot done as a BUBBLE PASS, not a max-search.
///
/// C's pivot step walks `i` from `n - 1` down to `k + 1` swapping ADJACENT
/// rows whenever the lower one has the larger magnitude. That is a bubble
/// pass, and it is NOT the same permutation a "find the max and swap once"
/// pivot produces when three or more rows are involved — different row order
/// means different rounding, so the two disagree in the last bits. Transcribed
/// as written.
///
/// Returns `false` (C's 0) when a pivot is degenerate.
pub fn linsolve(n: usize, a: &mut [f64], stride: usize, b: &mut [f64], x: &mut [f64]) -> bool {
    // Forward elimination.
    for k in 0..n.saturating_sub(1) {
        // Bring the largest magnitude to the diagonal position.
        let mut i = n - 1;
        while i > k {
            if a[(i - 1) * stride + k].abs() < a[i * stride + k].abs() {
                for j in 0..n {
                    a.swap(i * stride + j, (i - 1) * stride + j);
                }
                b.swap(i, i - 1);
            }
            i -= 1;
        }
        for i in k..n - 1 {
            if a[k * stride + k].abs() < TINY_NEAR_ZERO {
                return false;
            }
            let c = a[(i + 1) * stride + k] / a[k * stride + k];
            for j in 0..n {
                a[(i + 1) * stride + j] -= c * a[k * stride + j];
            }
            b[i + 1] -= c * b[k];
        }
    }
    // Backward substitution.
    for i in (0..n).rev() {
        if a[i * stride + i].abs() < TINY_NEAR_ZERO {
            return false;
        }
        let mut c = 0.0f64;
        for j in i + 1..n {
            c += a[i * stride + j] * x[j];
        }
        x[i] = (b[i] - c) / a[i * stride + i];
    }
    true
}

/// Port of `least_squares_accumulate` (mathutils.h:95) — accumulate `A'A` into
/// `mat` and `A'b` into `y` for one equation.
#[inline]
pub fn least_squares_accumulate(mat: &mut [f64], y: &mut [f64], a: &[f64], b: f64, n: usize) {
    for i in 0..n {
        for j in 0..n {
            mat[i * n + j] += a[i] * a[j];
        }
    }
    for i in 0..n {
        y[i] += a[i] * b;
    }
}

/// Port of `least_squares_solve` (mathutils.h:106).
#[inline]
pub fn least_squares_solve(mat: &mut [f64], y: &mut [f64], x: &mut [f64], n: usize) -> bool {
    linsolve(n, mat, n, y, x)
}

// --------------------------------------------------------------------------
// ransac.c
// --------------------------------------------------------------------------

/// `RANSAC_MOTION` (ransac.h:26).
#[derive(Debug, Clone, Default)]
struct RansacMotion {
    num_inliers: usize,
    sse: f64,
    inlier_indices: alloc::vec::Vec<i32>,
}

/// Port of `compare_motions` (ransac.c:39): more inliers wins; on a tie, lower
/// SSE wins; otherwise equal.
fn compare_motions(a: &RansacMotion, b: &RansacMotion) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    if a.num_inliers > b.num_inliers {
        return Ordering::Less;
    }
    if a.num_inliers < b.num_inliers {
        return Ordering::Greater;
    }
    if a.sse < b.sse {
        return Ordering::Less;
    }
    if a.sse > b.sse {
        return Ordering::Greater;
    }
    Ordering::Equal
}

/// Port of `is_better_motion` (ransac.c:58).
#[inline]
fn is_better_motion(a: &RansacMotion, b: &RansacMotion) -> bool {
    compare_motions(a, b) == core::cmp::Ordering::Less
}

/// Port of `score_translation` (ransac.c:62).
fn score_translation(
    mat: &[f64; MAX_PARAMDIM],
    points: &[Correspondence],
    model: &mut RansacMotion,
) {
    model.num_inliers = 0;
    model.sse = 0.0;
    for (i, p) in points.iter().enumerate() {
        let x1 = f64::from(p.x);
        let y1 = f64::from(p.y);
        let x2 = f64::from(p.rx);
        let y2 = f64::from(p.ry);
        let dx = (x1 + mat[0]) - x2;
        let dy = (y1 + mat[1]) - y2;
        let sse = dx * dx + dy * dy;
        if sse < INLIER_THRESHOLD_SQUARED {
            model.inlier_indices[model.num_inliers] = i as i32;
            model.num_inliers += 1;
            model.sse += sse;
        }
    }
}

/// Port of `score_affine` (ransac.c:86).
fn score_affine(mat: &[f64; MAX_PARAMDIM], points: &[Correspondence], model: &mut RansacMotion) {
    model.num_inliers = 0;
    model.sse = 0.0;
    for (i, p) in points.iter().enumerate() {
        let x1 = f64::from(p.x);
        let y1 = f64::from(p.y);
        let x2 = f64::from(p.rx);
        let y2 = f64::from(p.ry);
        let dx = (mat[2] * x1 + mat[3] * y1 + mat[0]) - x2;
        let dy = (mat[4] * x1 + mat[5] * y1 + mat[1]) - y2;
        let sse = dx * dx + dy * dy;
        if sse < INLIER_THRESHOLD_SQUARED {
            model.inlier_indices[model.num_inliers] = i as i32;
            model.num_inliers += 1;
            model.sse += sse;
        }
    }
}

/// Port of `find_translation` (ransac.c:110). Always succeeds.
fn find_translation(
    points: &[Correspondence],
    indices: &[i32],
    num_indices: usize,
    params: &mut [f64; MAX_PARAMDIM],
) -> bool {
    let mut sumx = 0.0f64;
    let mut sumy = 0.0f64;
    for &idx in &indices[..num_indices] {
        let p = &points[idx as usize];
        sumx += f64::from(p.rx) - f64::from(p.x);
        sumy += f64::from(p.ry) - f64::from(p.y);
    }
    params[0] = sumx / num_indices as f64;
    params[1] = sumy / num_indices as f64;
    params[2] = 1.0;
    params[3] = 0.0;
    params[4] = 0.0;
    params[5] = 1.0;
    true
}

/// Port of `find_rotzoom` (ransac.c:134) — a 4-dimensional least-squares
/// solve, TWO equations per correspondence (one for `dx`, one for `dy`), with
/// the second row's coefficients `[0, 1, sy, -sx]` encoding the rotzoom
/// constraint. `params[4] = -params[3]`, `params[5] = params[2]` afterwards.
fn find_rotzoom(
    points: &[Correspondence],
    indices: &[i32],
    num_indices: usize,
    params: &mut [f64; MAX_PARAMDIM],
) -> bool {
    const N: usize = 4;
    let mut mat = [0.0f64; N * N];
    let mut y = [0.0f64; N];
    for &idx in &indices[..num_indices] {
        let p = &points[idx as usize];
        let (sx, sy) = (f64::from(p.x), f64::from(p.y));
        let (dx, dy) = (f64::from(p.rx), f64::from(p.ry));

        let a = [1.0, 0.0, sx, sy];
        least_squares_accumulate(&mut mat, &mut y, &a, dx, N);

        let a = [0.0, 1.0, sy, -sx];
        least_squares_accumulate(&mut mat, &mut y, &a, dy, N);
    }
    let mut x = [0.0f64; N];
    if !least_squares_solve(&mut mat, &mut y, &mut x, N) {
        return false;
    }
    params[..N].copy_from_slice(&x);
    params[4] = -params[3];
    params[5] = params[2];
    true
}

/// Port of `find_affine` (ransac.c:175).
///
/// C splits the 6-dimensional problem into TWO INDEPENDENT 3-dimensional ones
/// (the x-output parameters and the y-output parameters) and recombines. That
/// is not just an optimisation — solving the 6-dim system directly would pivot
/// differently and give different last bits — so the split is part of the
/// port.
fn find_affine(
    points: &[Correspondence],
    indices: &[i32],
    num_indices: usize,
    params: &mut [f64; MAX_PARAMDIM],
) -> bool {
    const N: usize = 3;
    let mut mat = [[0.0f64; N * N]; 2];
    let mut y = [[0.0f64; N]; 2];
    for &idx in &indices[..num_indices] {
        let p = &points[idx as usize];
        let (sx, sy) = (f64::from(p.x), f64::from(p.y));
        let (dx, dy) = (f64::from(p.rx), f64::from(p.ry));
        let a = [1.0, sx, sy];
        least_squares_accumulate(&mut mat[0], &mut y[0], &a, dx, N);
        least_squares_accumulate(&mut mat[1], &mut y[1], &a, dy, N);
    }
    let mut x = [[0.0f64; N]; 2];
    let (m0, m1) = mat.split_at_mut(1);
    let (y0, y1) = y.split_at_mut(1);
    let (x0, x1) = x.split_at_mut(1);
    if !least_squares_solve(&mut m0[0], &mut y0[0], &mut x0[0], N) {
        return false;
    }
    if !least_squares_solve(&mut m1[0], &mut y1[0], &mut x1[0], N) {
        return false;
    }
    params[0] = x0[0][0];
    params[1] = x1[0][0];
    params[2] = x0[0][1];
    params[3] = x0[0][2];
    params[4] = x1[0][1];
    params[5] = x1[0][2];
    true
}

/// `TransformationType` as `svt_aom_ransac` takes it. IDENTITY is rejected by
/// C's own assert (`type > IDENTITY && type < TRANS_TYPES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RansacModel {
    Translation,
    RotZoom,
    Affine,
}

impl RansacModel {
    /// `RansacModelInfo::minpts` (ransac.c:416).
    #[inline]
    const fn minpts(self) -> usize {
        match self {
            RansacModel::Translation => 1,
            RansacModel::RotZoom => 2,
            RansacModel::Affine => 3,
        }
    }
    fn find_transformation(
        self,
        points: &[Correspondence],
        indices: &[i32],
        num_indices: usize,
        params: &mut [f64; MAX_PARAMDIM],
    ) -> bool {
        match self {
            RansacModel::Translation => find_translation(points, indices, num_indices, params),
            RansacModel::RotZoom => find_rotzoom(points, indices, num_indices, params),
            RansacModel::Affine => find_affine(points, indices, num_indices, params),
        }
    }
    fn score_model(
        self,
        params: &[f64; MAX_PARAMDIM],
        points: &[Correspondence],
        model: &mut RansacMotion,
    ) {
        match self {
            // ROTZOOM and AFFINE both score with score_affine (ransac.c:416).
            RansacModel::Translation => score_translation(params, points, model),
            RansacModel::RotZoom | RansacModel::Affine => score_affine(params, points, model),
        }
    }
}

/// Port of `svt_aom_ransac` / `ransac_internal` (ransac.c:428 / :234).
///
/// Returns `false` on the "not enough points" early-out, matching C. The
/// output models are pre-initialised to the identity model, so a `false`
/// return still leaves a usable (identity) result — which is what
/// `determine_gm_params` relies on.
///
/// C's allocation-failure path is not represented: the port's buffers are
/// `Vec`s, so the `mem_alloc_failed` out-parameter has no counterpart.
pub fn ransac(
    matched_points: &[Correspondence],
    model: RansacModel,
    num_desired_motions: usize,
) -> (bool, alloc::vec::Vec<MotionModel>) {
    let npoints = matched_points.len();
    let minpts = model.minpts();

    let mut motion_models: alloc::vec::Vec<MotionModel> = (0..num_desired_motions)
        .map(|_| MotionModel::default())
        .collect();

    if npoints < minpts * MINPTS_MULTIPLIER || npoints == 0 {
        return (false, motion_models);
    }

    // C: `AOMMAX((int)(MIN_INLIER_PROB * npoints), minpts)` — the cast
    // TRUNCATES toward zero, it does not round.
    let min_inliers = ((MIN_INLIER_PROB * npoints as f64) as i32).max(minpts as i32) as usize;

    let mut seed: u32 = npoints as u32;
    let mut indices = [0i32; MAX_MINPTS];

    let mut motions: alloc::vec::Vec<RansacMotion> = (0..num_desired_motions)
        .map(|_| RansacMotion {
            num_inliers: 0,
            sse: 0.0,
            inlier_indices: alloc::vec![0i32; npoints],
        })
        .collect();
    let mut current_motion = RansacMotion {
        num_inliers: 0,
        sse: 0.0,
        inlier_indices: alloc::vec![0i32; npoints],
    };

    // C tracks `worst_kept_motion` as a POINTER into `motions`; the port keeps
    // the index instead. It starts at 0, not at the true worst — C does the
    // same, and the first replacement recomputes it.
    let mut worst = 0usize;
    let mut params_this_motion = [0.0f64; MAX_PARAMDIM];

    for _ in 0..NUM_TRIALS {
        lcg_pick(npoints, minpts, &mut indices, &mut seed);
        if !model.find_transformation(matched_points, &indices, minpts, &mut params_this_motion) {
            continue;
        }
        model.score_model(&params_this_motion, matched_points, &mut current_motion);
        if current_motion.num_inliers < min_inliers {
            continue;
        }
        if is_better_motion(&current_motion, &motions[worst]) {
            motions[worst].num_inliers = current_motion.num_inliers;
            motions[worst].sse = current_motion.sse;
            // C swaps the index buffers rather than copying; the port does the
            // same, which also means `current_motion`'s buffer is scratch from
            // here on (its previous contents are overwritten next trial).
            core::mem::swap(
                &mut motions[worst].inlier_indices,
                &mut current_motion.inlier_indices,
            );
            for i in 0..num_desired_motions {
                if is_better_motion(&motions[worst], &motions[i]) {
                    worst = i;
                }
            }
        }
    }

    // Sort the motions, best first. See the module doc on the tie hazard: C
    // uses qsort (unstable, implementation-defined on ties); this is a STABLE
    // sort.
    motions.sort_by(compare_motions);

    for i in 0..num_desired_motions {
        if motions[i].num_inliers == 0 {
            // Already initialised to the identity model.
            continue;
        }
        let mut bad_model = false;
        for _ in 0..NUM_REFINES {
            let num_inliers = motions[i].num_inliers;
            if !model.find_transformation(
                matched_points,
                &motions[i].inlier_indices,
                num_inliers,
                &mut params_this_motion,
            ) {
                bad_model = true;
                break;
            }
            model.score_model(&params_this_motion, matched_points, &mut current_motion);
            if current_motion.num_inliers > motions[i].num_inliers {
                motions[i].num_inliers = current_motion.num_inliers;
                motions[i].sse = current_motion.sse;
                core::mem::swap(
                    &mut motions[i].inlier_indices,
                    &mut current_motion.inlier_indices,
                );
            } else {
                // Refined model is no better, so stop — and KEEP
                // params_this_motion, which is what gets written out. C relies
                // on that too rather than caching the previous iteration.
                break;
            }
        }
        if bad_model {
            continue;
        }
        motion_models[i].params = params_this_motion;
        motion_models[i].inliers.clear();
        for j in 0..motions[i].num_inliers {
            let corr = &matched_points[motions[i].inlier_indices[j] as usize];
            // C applies rint() to fields that are already `int`, so this is
            // exact; kept as a plain widening rather than pretending there is
            // rounding to reproduce.
            motion_models[i].inliers.push(corr.x);
            motion_models[i].inliers.push(corr.y);
        }
        motion_models[i].num_inliers = motions[i].num_inliers;
    }

    (true, motion_models)
}

/// Port of `determine_gm_params` (global_motion.c:364) — a one-line wrapper
/// over [`ransac`], kept as its own function so the C call graph maps 1:1.
pub fn determine_gm_params(
    model: RansacModel,
    correspondences: &[Correspondence],
    num_desired_motions: usize,
) -> alloc::vec::Vec<MotionModel> {
    ransac(correspondences, model, num_desired_motions).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_points_returns_identity_models() {
        // minpts * MINPTS_MULTIPLIER is 5 / 10 / 15 for the three models.
        for (m, need) in [
            (RansacModel::Translation, 5usize),
            (RansacModel::RotZoom, 10),
            (RansacModel::Affine, 15),
        ] {
            let pts = alloc::vec![Correspondence::default(); need - 1];
            let (ok, models) = ransac(&pts, m, 1);
            assert!(!ok, "{m:?} with {} points must early-out", need - 1);
            assert_eq!(models[0].params, IDENTITY_PARAMS);
            assert_eq!(models[0].num_inliers, 0);
        }
    }

    #[test]
    fn lcg_pick_returns_distinct_indices_in_range() {
        let mut seed = 40u32;
        let mut out = [0i32; MAX_MINPTS];
        for _ in 0..200 {
            lcg_pick(40, 4, &mut out, &mut seed);
            for (i, &v) in out.iter().enumerate() {
                assert!((0..40).contains(&v));
                assert!(!out[..i].contains(&v), "repeated index in {out:?}");
            }
        }
    }

    #[test]
    fn linsolve_rejects_a_singular_system() {
        // Two identical rows -> singular.
        let mut a = [1.0, 2.0, 1.0, 2.0];
        let mut b = [3.0, 3.0];
        let mut x = [0.0; 2];
        assert!(!linsolve(2, &mut a, 2, &mut b, &mut x));
    }
}
