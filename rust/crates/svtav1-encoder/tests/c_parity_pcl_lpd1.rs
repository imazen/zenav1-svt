//! Differential parity: the chroma-complexity detector
//! (`svtav1-encoder/src/port_md/lpd1.rs`) vs the REAL exported C.
//!
//! **Evidence tier 1** (`docs/WORKING-ON-THIS.md` §4):
//!
//! | oracle | C |
//! |---|---|
//! | `chroma_complexity_check_pred` | product_coding_loop.c:6013 |
//!
//! `nm -g` reports `T _chroma_complexity_check_pred` — no `svt_aom_`
//! prefix, and no prototype in any header, so `shims/pcl_shims.c` declares
//! it. Its variance arm dispatches through `svt_aom_mefn_ptr[..].vf`, whose
//! entries are null until `init_fn_ptr` runs — the shim's header documents
//! that two-level RTCD trap (§5 trap 2), which was MEASURED here as a
//! SIGSEGV before the init call was added.
//!
//! # Why the content is constructed, not random
//!
//! The first version of this file drew random planes at four spreads over
//! eight geometries and four priors — 1,024 cases, all green. It was
//! **vacuous**: with independent random input and prediction, every plane's
//! SAD lands within a factor of ~1.2 of the luma one, so `cb_dist > 2 *
//! y_dist` is never true and every case returned `COMPONENT_LUMA`.
//! Mutating the port's `y_dist << 1` to `<< 2` and its variance threshold
//! from 150 to 75 left all 1,024 cases passing.
//!
//! So the planes below are BUILT to straddle each threshold, and
//! [`outcome_census`] asserts the grid actually produced every outcome —
//! the positive control §5 requires before a green run means anything.
//! Each mutation above now fails.
//!
//! The other functions in `port_md::lpd1` are `static` in C and are tier 4;
//! their vectors are in that module's own test block.

use std::collections::BTreeSet;

use svtav1_cref::pcl as cpcl;
use svtav1_encoder::port_md::lpd1 as rl;

/// `(bsize_uv as C BlockSize, bwidth_uv, bheight_uv)`.
///
/// `bsize_uv` selects `eb_num_pels_log2_lookup[bsize_uv]`, which normalises
/// the variance, and `bheight_uv` selects the row-subsampling shift at the
/// 4 and 8 boundaries — so the grid spans both.
const GEOMS: &[(usize, usize, usize)] = &[
    (0, 4, 4),   // BLOCK_4X4   — shift 0
    (1, 4, 8),   // BLOCK_4X8   — shift 1
    (2, 8, 4),   // BLOCK_8X4   — shift 0
    (3, 8, 8),   // BLOCK_8X8   — shift 1
    (4, 8, 16),  // BLOCK_8X16  — shift 2
    (5, 16, 8),  // BLOCK_16X8  — shift 1
    (6, 16, 16), // BLOCK_16X16 — shift 2
    (9, 32, 32), // BLOCK_32X32 — shift 2
];

const STRIDE: usize = 64;
const PLANE_LEN: usize = STRIDE * 64;

const PRIORS: [rl::ComponentType; 4] = [
    rl::ComponentType::Luma,
    rl::ComponentType::Chroma,
    rl::ComponentType::Cb,
    rl::ComponentType::Cr,
];

fn component_of(raw: i32) -> rl::ComponentType {
    match raw {
        0 => rl::ComponentType::Luma,
        1 => rl::ComponentType::Chroma,
        2 => rl::ComponentType::Cb,
        3 => rl::ComponentType::Cr,
        other => panic!("C returned an unexpected COMPONENT_TYPE {other}"),
    }
}

struct Case<'a> {
    prior_chroma: rl::ComponentType,
    prior_cfl: rl::ComponentType,
    geom: (usize, usize, usize),
    input: [&'a [u8]; 3],
    pred: [&'a [u8]; 3],
    use_var: bool,
    cfl_cplx_th: u32,
}

/// Run one case through both sides and return C's `(chroma, cfl)` so the
/// caller can census the outcomes.
fn check(c: &Case<'_>) -> (rl::ComponentType, rl::ComponentType) {
    let (bsize_uv, bw, bh) = c.geom;
    let (c_chroma, c_cfl) = cpcl::chroma_complexity_check_pred(
        c.prior_chroma as i32,
        c.prior_cfl as i32,
        bw,
        bh,
        bsize_uv,
        [
            cpcl::RefPlane {
                data: c.input[0],
                stride: STRIDE,
            },
            cpcl::RefPlane {
                data: c.input[1],
                stride: STRIDE,
            },
            cpcl::RefPlane {
                data: c.input[2],
                stride: STRIDE,
            },
        ],
        [
            cpcl::RefPlane {
                data: c.pred[0],
                stride: STRIDE,
            },
            cpcl::RefPlane {
                data: c.pred[1],
                stride: STRIDE,
            },
            cpcl::RefPlane {
                data: c.pred[2],
                stride: STRIDE,
            },
        ],
        c.use_var,
        c.cfl_cplx_th,
    )
    .expect("shim allocation must succeed");

    let got = rl::chroma_complexity_check_pred(
        rl::ChromaState {
            chroma_complexity: c.prior_chroma,
            cfl_complexity: c.prior_cfl,
        },
        rl::UvGeom {
            bwidth_uv: bw,
            bheight_uv: bh,
            bsize_uv,
        },
        rl::BlockPlanes {
            y: rl::Plane::new(c.input[0], STRIDE),
            u: rl::Plane::new(c.input[1], STRIDE),
            v: rl::Plane::new(c.input[2], STRIDE),
        },
        rl::BlockPlanes {
            y: rl::Plane::new(c.pred[0], STRIDE),
            u: rl::Plane::new(c.pred[1], STRIDE),
            v: rl::Plane::new(c.pred[2], STRIDE),
        },
        c.use_var,
        c.cfl_cplx_th,
    );
    let expected = (component_of(c_chroma), component_of(c_cfl));
    assert_eq!(
        (got.chroma_complexity, got.cfl_complexity),
        expected,
        "geom {:?} prior {:?}/{:?} use_var {} th {}",
        c.geom,
        c.prior_chroma,
        c.prior_cfl,
        c.use_var,
        c.cfl_cplx_th
    );
    expected
}

/// Every outcome the whole file observed from C. A grid that cannot produce
/// all of them is not testing the ladder it claims to test.
#[derive(Default)]
struct Census(BTreeSet<(u8, u8)>);

impl Census {
    fn record(&mut self, o: (rl::ComponentType, rl::ComponentType)) {
        self.0.insert((o.0 as u8, o.1 as u8));
    }
    fn assert_saw_every_chroma_outcome(&self) {
        for want in 0u8..4 {
            assert!(
                self.0.iter().any(|&(c, _)| c == want),
                "grid never produced chroma outcome {want}; seen {:?}",
                self.0
            );
        }
        assert!(
            self.0.iter().any(|&(_, f)| f == 1),
            "grid never set cfl_complexity; seen {:?}",
            self.0
        );
        assert!(
            self.0.iter().any(|&(_, f)| f == 0),
            "grid never left cfl_complexity clear; seen {:?}",
            self.0
        );
    }
}

/// A plane of a single value.
fn flat(v: u8) -> Vec<u8> {
    vec![v; PLANE_LEN]
}

/// A column-striped plane centred on 128 with amplitude `d`: half the
/// samples at `128 + d`, half at `128 - d`, so `sum == 0` and the
/// normalised variance is very close to `d * d`.
///
/// A CONSTANT plane has variance ZERO (C's `variance_c` subtracts
/// `sum^2 / n`), so constant content cannot exercise the variance arm at
/// all — which is half of why the random grid was vacuous.
fn striped(d: u8) -> Vec<u8> {
    (0..PLANE_LEN)
        .map(|i| if i % 2 == 0 { 128 + d } else { 128 - d })
        .collect()
}

/// A plane whose value depends on the row index mod 4, so sampling every
/// row / every 2nd / every 4th yields three different means. This is what
/// makes the subsampling shift observable.
fn row_pattern(vals: [u8; 4]) -> Vec<u8> {
    (0..PLANE_LEN).map(|i| vals[(i / STRIDE) % 4]).collect()
}

/// The SAD ladder: luma is weighted by 2, so a chroma plane must exceed
/// TWICE the luma error. Amplitudes are swept so the ratio straddles 2
/// (and 4, which is what the `<< 2` mutation would need).
#[test]
fn luma_weighting_and_the_dominance_ladder_match_c() {
    let mut census = Census::default();
    let zero = flat(0);
    for &geom in GEOMS {
        for y_amp in [1u8, 4, 10] {
            let pred_y = flat(y_amp);
            for u_amp in [0u8, 1, 2, 4, 8, 16, 40] {
                for v_amp in [0u8, 1, 2, 4, 8, 16, 40] {
                    let pred_u = flat(u_amp);
                    let pred_v = flat(v_amp);
                    for &prior in &PRIORS {
                        census.record(check(&Case {
                            prior_chroma: prior,
                            prior_cfl: rl::ComponentType::Luma,
                            geom,
                            input: [&zero, &zero, &zero],
                            pred: [&pred_y, &pred_u, &pred_v],
                            use_var: false,
                            cfl_cplx_th: 0,
                        }));
                    }
                }
            }
        }
    }
    census.assert_saw_every_chroma_outcome();
}

/// The variance arm: threshold 150 on a `d^2`-ish statistic, so the sweep
/// straddles it at d = 12 (144) and d = 13 (169). `cfl_cplx_th` is swept
/// across the same range because it reads the SAME two variances.
#[test]
fn variance_threshold_and_cfl_threshold_match_c() {
    let mut census = Census::default();
    let flat128 = flat(128);
    for &geom in GEOMS {
        for u_d in [0u8, 8, 12, 13, 20, 40] {
            for v_d in [0u8, 8, 12, 13, 20, 40] {
                let in_u = striped(u_d);
                let in_v = striped(v_d);
                for &th in &[0u32, 60, 144, 150, 169, 255] {
                    for &prior in &PRIORS {
                        census.record(check(&Case {
                            prior_chroma: prior,
                            prior_cfl: rl::ComponentType::Luma,
                            geom,
                            // Prediction equals input on every plane, so the
                            // SAD arm contributes nothing and only the
                            // variance arm can move the result.
                            input: [&flat128, &in_u, &in_v],
                            pred: [&flat128, &in_u, &in_v],
                            use_var: true,
                            cfl_cplx_th: th,
                        }));
                    }
                }
            }
        }
    }
    census.assert_saw_every_chroma_outcome();
}

/// The row-subsampling shift (`bheight_uv > 8 ? 2 : > 4 ? 1 : 0`). The
/// planes differ per row mod 4, so a wrong shift measures different rows
/// and lands on a different side of the 2x luma weight.
#[test]
fn row_subsampling_shift_matches_c() {
    let mut census = Census::default();
    let zero = flat(0);
    // Row means: every row 10, every 2nd row 4, every 4th row 2.
    let pred_y = row_pattern([2, 18, 6, 14]);
    for &geom in GEOMS {
        for u_amp in [0u8, 3, 5, 9, 13, 21, 41] {
            for v_amp in [0u8, 3, 5, 9, 13, 21, 41] {
                let pred_u = flat(u_amp);
                let pred_v = flat(v_amp);
                for &prior in &PRIORS {
                    census.record(check(&Case {
                        prior_chroma: prior,
                        prior_cfl: rl::ComponentType::Luma,
                        geom,
                        input: [&zero, &zero, &zero],
                        pred: [&pred_y, &pred_u, &pred_v],
                        use_var: false,
                        cfl_cplx_th: 0,
                    }));
                }
            }
        }
    }
    census.assert_saw_every_chroma_outcome();
}

/// A prior of `COMPONENT_CHROMA` must return IMMEDIATELY — in particular
/// `cfl_complexity` must NOT be updated, even on content that would set it.
/// Content is chosen so that falling through WOULD set it, so a port
/// without the early return diverges here rather than agreeing by accident.
#[test]
fn a_chroma_prior_short_circuits_without_touching_cfl() {
    let zero = flat(0);
    let big = flat(60);
    let busy = striped(40);
    let flat128 = flat(128);
    for &geom in GEOMS {
        // SAD arm would fire: chroma error 60 vs luma 0.
        let seen = check(&Case {
            prior_chroma: rl::ComponentType::Chroma,
            prior_cfl: rl::ComponentType::Luma,
            geom,
            input: [&zero, &zero, &zero],
            pred: [&zero, &big, &big],
            use_var: true,
            cfl_cplx_th: 0,
        });
        assert_eq!(
            seen,
            (rl::ComponentType::Chroma, rl::ComponentType::Luma),
            "the early return must leave cfl_complexity alone"
        );
        // Variance arm would fire too.
        check(&Case {
            prior_chroma: rl::ComponentType::Chroma,
            prior_cfl: rl::ComponentType::Luma,
            geom,
            input: [&flat128, &busy, &busy],
            pred: [&flat128, &busy, &busy],
            use_var: true,
            cfl_cplx_th: 0,
        });
    }
}

/// Both arms live at once: the SAD arm flags one plane and the variance arm
/// flags the other, which is the only way to reach `COMPONENT_CHROMA`
/// through the two-step merge rather than from a single arm.
#[test]
fn the_two_arms_compose_through_the_merge() {
    let mut census = Census::default();
    let flat128 = flat(128);
    let busy = striped(30);
    for &geom in GEOMS {
        for (u_in, u_pred) in [(&flat128, &flat128), (&busy, &busy)] {
            for v_pred_amp in [128u8, 190] {
                let v_pred = flat(v_pred_amp);
                for &prior in &PRIORS {
                    census.record(check(&Case {
                        prior_chroma: prior,
                        prior_cfl: rl::ComponentType::Luma,
                        geom,
                        input: [&flat128, u_in, &flat128],
                        pred: [&flat128, u_pred, &v_pred],
                        use_var: true,
                        cfl_cplx_th: 200,
                    }));
                }
            }
        }
    }
    census.assert_saw_every_chroma_outcome();
}
