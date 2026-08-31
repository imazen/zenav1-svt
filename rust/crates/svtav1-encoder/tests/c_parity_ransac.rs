//! Differential (evidence tier 1): `port_ransac` against the REAL exported
//! `svt_aom_ransac` (`Codec/ransac.c:428`).
//!
//! One oracle covers the whole file: `svt_aom_ransac` drives every static in
//! it — `compare_motions`, `is_better_motion`, `score_translation`,
//! `score_affine`, `find_translation`, `find_rotzoom`, `find_affine`,
//! `ransac_internal` — plus the `random.h` PRNG (`lcg_next`, `lcg_randint`,
//! `lcg_pick`) and the `mathutils.h` solver (`linsolve`,
//! `least_squares_accumulate`, `least_squares_solve`) it inlines.
//!
//! The comparison on `params` is EXACT (`f64` bit equality), not approximate.
//! Every step is `f64` in both, accumulated in the same order, and an
//! approximate comparison would hide exactly the reordering bugs this suite
//! exists to catch.

use svtav1_cref as cref;
use svtav1_encoder::port_ransac::{Correspondence, RansacModel, ransac};

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 32) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u32() % ((hi - lo + 1) as u32)) as i32
    }
}

fn flat(points: &[Correspondence]) -> Vec<i32> {
    points.iter().flat_map(|p| [p.x, p.y, p.rx, p.ry]).collect()
}

fn ty_of(m: RansacModel) -> i32 {
    match m {
        RansacModel::Translation => 1,
        RansacModel::RotZoom => 2,
        RansacModel::Affine => 3,
    }
}

/// Build a correspondence set that really contains the given affine model,
/// plus `outliers` points that do not, so the inlier/outlier split is
/// non-trivial and the refinement loop has something to converge on.
fn synth(n: usize, outliers: usize, m: [f64; 6], seed: u64, grid: i32) -> Vec<Correspondence> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = rng.range(0, grid);
        let y = rng.range(0, grid);
        if i < outliers {
            out.push(Correspondence {
                x,
                y,
                rx: rng.range(0, grid),
                ry: rng.range(0, grid),
            });
        } else {
            let rx = (m[2] * f64::from(x) + m[3] * f64::from(y) + m[0]).round() as i32;
            let ry = (m[4] * f64::from(x) + m[5] * f64::from(y) + m[1]).round() as i32;
            out.push(Correspondence { x, y, rx, ry });
        }
    }
    out
}

fn assert_matches_c(
    points: &[Correspondence],
    model: RansacModel,
    ndm: usize,
    what: &str,
) -> usize {
    let (c_ok, c_models) = cref::ransac(&flat(points), ty_of(model), ndm);
    let (r_ok, r_models) = ransac(points, model, ndm);
    assert_eq!(r_ok, c_ok, "{what}: return value");
    assert_eq!(r_models.len(), c_models.len());
    let mut total_inliers = 0usize;
    for (i, (r, c)) in r_models.iter().zip(c_models.iter()).enumerate() {
        assert_eq!(
            r.num_inliers, c.num_inliers,
            "{what}: model {i} num_inliers"
        );
        // Exact f64 equality — see the module doc.
        assert_eq!(r.params, c.params, "{what}: model {i} params");
        assert_eq!(
            r.inliers[..2 * c.num_inliers],
            c.inliers[..2 * c.num_inliers],
            "{what}: model {i} inlier points"
        );
        total_inliers += c.num_inliers;
    }
    total_inliers
}

/// The "not enough points" early-out, for each model's own `minpts *
/// MINPTS_MULTIPLIER` threshold.
#[test]
fn ransac_early_out_matches_c() {
    for (m, need) in [
        (RansacModel::Translation, 5usize),
        (RansacModel::RotZoom, 10),
        (RansacModel::Affine, 15),
    ] {
        for n in [0usize, 1, need - 1] {
            let pts = synth(n, 0, [0.0, 0.0, 1.0, 0.0, 0.0, 1.0], 1, 100);
            assert_matches_c(&pts, m, 1, &format!("early-out {m:?} n={n}"));
        }
        // And one point above the threshold must NOT early-out.
        let pts = synth(need, 0, [3.0, -2.0, 1.0, 0.0, 0.0, 1.0], 2, 100);
        let (c_ok, _) = cref::ransac(&flat(&pts), ty_of(m), 1);
        assert!(c_ok, "{m:?} with {need} points must not early-out");
    }
}

/// Clean data containing exactly the model being fitted.
#[test]
fn ransac_matches_c_on_clean_models() {
    let cases: [(RansacModel, [f64; 6]); 3] = [
        (RansacModel::Translation, [7.0, -5.0, 1.0, 0.0, 0.0, 1.0]),
        // A rotzoom: mat[5] == mat[2], mat[4] == -mat[3].
        (RansacModel::RotZoom, [3.0, 4.0, 1.0, 0.0, 0.0, 1.0]),
        (RansacModel::Affine, [2.0, -1.0, 1.0, 0.0, 0.0, 1.0]),
    ];
    for (model, m) in cases {
        for &n in &[20usize, 64, 200] {
            let pts = synth(n, 0, m, n as u64 * 7, 200);
            let inl = assert_matches_c(&pts, model, 1, &format!("clean {model:?} n={n}"));
            assert!(
                inl > 0,
                "clean {model:?} n={n} found no inliers — the oracle is not being exercised"
            );
        }
    }
}

/// Data with a real outlier fraction, which is what makes the trial loop, the
/// `min_inliers` reject, the worst-kept bookkeeping and the refinement all
/// matter.
#[test]
fn ransac_matches_c_with_outliers() {
    let mut exercised = 0usize;
    for model in [
        RansacModel::Translation,
        RansacModel::RotZoom,
        RansacModel::Affine,
    ] {
        for &(n, outliers) in &[
            (40usize, 5usize),
            (60, 20),
            (100, 40),
            (100, 70),
            (200, 100),
            (30, 25),
        ] {
            let m = [5.0, -3.0, 1.0, 0.0, 0.0, 1.0];
            let pts = synth(n, outliers, m, (n * 31 + outliers) as u64, 300);
            let inl = assert_matches_c(
                &pts,
                model,
                1,
                &format!("outliers {model:?} n={n} out={outliers}"),
            );
            if inl > 0 {
                exercised += 1;
            }
        }
    }
    assert!(
        exercised > 10,
        "only {exercised} cells produced a fitted model — most cells early-out or \
         reject, so the search is barely tested"
    );
}

/// `num_desired_motions > 1` exercises the worst-kept-motion bookkeeping and
/// the sort, which are otherwise trivial with a single slot.
#[test]
fn ransac_matches_c_with_multiple_desired_motions() {
    for ndm in [1usize, 2, 4] {
        for model in [RansacModel::Translation, RansacModel::Affine] {
            let m = [4.0, 6.0, 1.0, 0.0, 0.0, 1.0];
            let pts = synth(120, 30, m, ndm as u64 * 13 + 1, 250);
            assert_matches_c(&pts, model, ndm, &format!("ndm {ndm} {model:?}"));
        }
    }
}

/// Degenerate geometry: all points collinear, or all identical. These are the
/// inputs that make `linsolve` return 0 and drive C's `bad_model` path.
#[test]
fn ransac_matches_c_on_degenerate_geometry() {
    // All points identical.
    let pts: Vec<Correspondence> = (0..40)
        .map(|_| Correspondence {
            x: 10,
            y: 10,
            rx: 12,
            ry: 12,
        })
        .collect();
    for model in [
        RansacModel::Translation,
        RansacModel::RotZoom,
        RansacModel::Affine,
    ] {
        assert_matches_c(&pts, model, 1, &format!("identical points {model:?}"));
    }

    // All points on a horizontal line -> the affine subproblems are singular.
    let pts: Vec<Correspondence> = (0..60)
        .map(|i| Correspondence {
            x: i,
            y: 0,
            rx: i + 3,
            ry: 0,
        })
        .collect();
    for model in [
        RansacModel::Translation,
        RansacModel::RotZoom,
        RansacModel::Affine,
    ] {
        assert_matches_c(&pts, model, 1, &format!("collinear points {model:?}"));
    }
}

/// Random correspondence sets with no underlying model at all — the case where
/// almost every trial is rejected by `min_inliers` and the output stays the
/// identity model. Confirms the port takes the same reject path C does rather
/// than inventing a model.
#[test]
fn ransac_matches_c_on_structureless_data() {
    let mut rng = Rng::new(0x5A5A);
    for trial in 0..30 {
        let n = 20 + (trial * 7) % 180;
        let pts: Vec<Correspondence> = (0..n)
            .map(|_| Correspondence {
                x: rng.range(0, 500),
                y: rng.range(0, 500),
                rx: rng.range(0, 500),
                ry: rng.range(0, 500),
            })
            .collect();
        for model in [
            RansacModel::Translation,
            RansacModel::RotZoom,
            RansacModel::Affine,
        ] {
            assert_matches_c(&pts, model, 1, &format!("noise trial {trial} {model:?}"));
        }
    }
}
