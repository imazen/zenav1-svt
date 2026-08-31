//! Differential (evidence tier 1): the global-motion model chain against the
//! REAL exported symbols of `Codec/global_motion.c` and
//! `Codec/enc_warped_motion.c`.
//!
//! | C symbol                             | port fn                  | also covers (static in C) |
//! |--------------------------------------|--------------------------|---------------------------|
//! | `svt_av1_convert_model_to_params`    | `convert_model_to_params`| `convert_to_params`, `get_wmtype` |
//! | `svt_av1_is_enough_erroradvantage`   | `is_enough_erroradvantage`| —                        |
//! | `svt_av1_warp_error`                 | `av1_warp_error`         | `warp_error`              |
//! | `svt_av1_refine_integerized_param`   | `refine_integerized_param`| `add_param_offset`, `force_wmtype`, `get_wmtype`, `warp_error` |

use svtav1_cref as cref;
use svtav1_dsp::port_warp::WARPEDMODEL_PREC_BITS;
use svtav1_encoder::port_global_motion::{
    self as gm, GM_ERRORADV_TR_0, GM_ERRORADV_TR_1, GM_ERRORADV_TR_2, GmRefineCtrls,
};
use svtav1_types::motion::{TransformationType, WarpedMotionParams};

const PREC: i32 = 1 << WARPEDMODEL_PREC_BITS;

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
    /// A double in [lo, hi).
    fn unit(&mut self) -> f64 {
        f64::from(self.next_u32()) / f64::from(u32::MAX)
    }
}

fn ty_from_i32(v: i32) -> TransformationType {
    match v {
        0 => TransformationType::Identity,
        1 => TransformationType::Translation,
        2 => TransformationType::RotZoom,
        3 => TransformationType::Affine,
        _ => unreachable!("unknown TransformationType {v}"),
    }
}

fn wm_io(wm: &WarpedMotionParams) -> [i32; 11] {
    [
        wm.wm_type as i32,
        wm.wmmat[0],
        wm.wmmat[1],
        wm.wmmat[2],
        wm.wmmat[3],
        wm.wmmat[4],
        wm.wmmat[5],
        i32::from(wm.alpha),
        i32::from(wm.beta),
        i32::from(wm.gamma),
        i32::from(wm.delta),
    ]
}

fn assert_wm_eq(rust: &WarpedMotionParams, c: &[i32; 11], what: &str) {
    assert_eq!(rust.wm_type as i32, c[0], "{what}: wmtype");
    assert_eq!(
        rust.wmmat,
        [c[1], c[2], c[3], c[4], c[5], c[6]],
        "{what}: wmmat"
    );
    assert_eq!(
        [
            i32::from(rust.alpha),
            i32::from(rust.beta),
            i32::from(rust.gamma),
            i32::from(rust.delta)
        ],
        [c[7], c[8], c[9], c[10]],
        "{what}: shear"
    );
}

// ---------------------------------------------------------------------------
// 1. svt_av1_convert_model_to_params (covers convert_to_params + get_wmtype)
// ---------------------------------------------------------------------------

#[test]
fn convert_model_to_params_matches_c_on_structured_models() {
    let cases: [[f64; 6]; 8] = [
        [0.0, 0.0, 1.0, 0.0, 0.0, 1.0],       // identity
        [3.0, -2.0, 1.0, 0.0, 0.0, 1.0],      // translation
        [1.5, 0.25, 1.01, 0.02, -0.02, 1.01], // rotzoom-shaped
        [0.0, 0.0, 1.01, 0.02, 0.03, 0.99],   // affine
        [1e6, -1e6, 1.0, 0.0, 0.0, 1.0],      // translation clamp
        [0.0, 0.0, 100.0, 0.0, 0.0, 100.0],   // diagonal clamp
        [0.0, 0.0, 1.0, 50.0, -50.0, 1.0],    // off-diagonal clamp
        // Exact .5 ties on both the translation and the alpha grids, which is
        // where `floor(x + 0.5)` and `round`/`rint` disagree.
        [
            2.5 / 64.0,
            -2.5 / 64.0,
            1.0 + 2.5 / 32768.0,
            -2.5 / 32768.0,
            2.5 / 32768.0,
            1.0 - 2.5 / 32768.0,
        ],
    ];
    for p in cases {
        let (c_ty, c_mat) = cref::convert_model_to_params(&p);
        let r = gm::convert_model_to_params(&p);
        assert_eq!(r.wm_type as i32, c_ty, "wmtype mismatch for {p:?}");
        assert_eq!(r.wmmat, c_mat, "wmmat mismatch for {p:?}");
        assert!(!r.invalid);
    }
}

#[test]
fn convert_model_to_params_matches_c_on_random_models() {
    let mut rng = Rng::new(0x60AD);
    let mut seen = [0usize; 4];
    for _ in 0..5000 {
        // Mix scales so the clamps fire sometimes and not others.
        let big = rng.next_u32().is_multiple_of(4);
        let ts = if big { 1e5 } else { 32.0 };
        let asc = if big { 10.0 } else { 0.05 };
        let p = [
            (rng.unit() - 0.5) * ts,
            (rng.unit() - 0.5) * ts,
            1.0 + (rng.unit() - 0.5) * asc,
            (rng.unit() - 0.5) * asc,
            (rng.unit() - 0.5) * asc,
            1.0 + (rng.unit() - 0.5) * asc,
        ];
        let (c_ty, c_mat) = cref::convert_model_to_params(&p);
        let r = gm::convert_model_to_params(&p);
        assert_eq!(r.wm_type as i32, c_ty, "wmtype mismatch for {p:?}");
        assert_eq!(r.wmmat, c_mat, "wmmat mismatch for {p:?}");
        seen[c_ty as usize] += 1;
    }
    // Anti-vacuity for get_wmtype: force each classification explicitly, since
    // random doubles essentially never land on IDENTITY / TRANSLATION /
    // ROTZOOM exactly.
    for (p, want) in [
        ([0.0, 0.0, 1.0, 0.0, 0.0, 1.0], 0i32),
        ([1.0, 1.0, 1.0, 0.0, 0.0, 1.0], 1),
        ([0.0, 0.0, 1.01, 0.02, -0.02, 1.01], 2),
    ] {
        let (c_ty, _) = cref::convert_model_to_params(&p);
        assert_eq!(c_ty, want, "expected classification {want} for {p:?}");
        seen[c_ty as usize] += 1;
    }
    assert!(
        seen.iter().all(|&n| n > 0),
        "not every wmtype was produced: {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. svt_av1_is_enough_erroradvantage
// ---------------------------------------------------------------------------

#[test]
fn is_enough_erroradvantage_matches_c() {
    let mut rng = Rng::new(0xE12A);
    let mut yes = 0usize;
    let mut no = 0usize;
    for ty in [GM_ERRORADV_TR_0, GM_ERRORADV_TR_1, GM_ERRORADV_TR_2] {
        // Cover both sides of both thresholds, including exact boundaries.
        for &adv in &[0.0f64, 0.44, 0.45, 0.5, 0.65, 0.7, 1.0] {
            for &cost in &[0i32, 1, 100, 20_000, 30_000, 100_000] {
                let c = cref::is_enough_erroradvantage(adv, cost, ty as i32);
                let r = gm::is_enough_erroradvantage(adv, cost, ty);
                assert_eq!(r, c, "erroradv mismatch adv {adv} cost {cost} ty {ty}");
                if c { yes += 1 } else { no += 1 }
            }
        }
        for _ in 0..500 {
            let adv = rng.unit() * 1.5;
            let cost = rng.range(0, 60_000);
            let c = cref::is_enough_erroradvantage(adv, cost, ty as i32);
            let r = gm::is_enough_erroradvantage(adv, cost, ty);
            assert_eq!(r, c, "erroradv mismatch adv {adv} cost {cost} ty {ty}");
        }
    }
    assert!(yes > 10 && no > 10, "one verdict never fired: {yes}/{no}");
}

// ---------------------------------------------------------------------------
// 3. svt_av1_warp_error (covers the static warp_error)
// ---------------------------------------------------------------------------

/// Trailing slack every reference plane handed to the C oracle must carry.
///
/// MEASURED 2026-08-31: `svt_av1_refine_integerized_param` reaches
/// `svt_warp_plane` -> the RTCD `svt_av1_warp_affine`, which on this host is
/// `svt_av1_warp_affine_neon_i8mm`. That kernel replicates edge pixels with
/// the `warp_pad_left` / `warp_pad_right` tables AFTER a full-width vector
/// load, so it READS PAST the last row of an exactly-sized plane — an
/// intermittent SIGBUS (`KERN_PROTECTION_FAILURE`, crash inside
/// `svt_av1_warp_affine_neon_i8mm`) depending on where the allocator put the
/// buffer. The scalar `svt_av1_warp_affine_c` clamps per sample and never
/// does this, which is why the `c_parity_warp_model` suite (which drives `_c`
/// directly) never saw it.
///
/// A real encoder's reference frame always has borders, so this is the
/// HARNESS supplying what the real caller supplies, not a relaxation: the
/// addressable pixel rectangle (`width` x `height` at `stride`) is unchanged
/// and the extra bytes are never read as pixels by either side.
const PLANE_SLACK: usize = 4096;

fn plane(w: usize, h: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut v: Vec<u8> = (0..w * h).map(|_| rng.range(0, 255) as u8).collect();
    v.resize(w * h + PLANE_SLACK, 0);
    v
}

/// Models C accepts as warp-legal, taken from C's own shear gate.
fn legal_models(n: usize) -> Vec<[i32; 6]> {
    let mut rng = Rng::new(0xC0DE);
    let mut out = vec![[0, 0, PREC, 0, 0, PREC]];
    while out.len() < n {
        let mat = [
            rng.range(-4 * PREC, 4 * PREC),
            rng.range(-4 * PREC, 4 * PREC),
            PREC + rng.range(-2000, 2000),
            rng.range(-2000, 2000),
            rng.range(-2000, 2000),
            PREC + rng.range(-2000, 2000),
        ];
        if cref::get_shear_params(&mat).0 {
            out.push(mat);
        }
    }
    out
}

#[test]
fn warp_error_matches_c_including_the_chess_pattern_and_early_out() {
    let (w, h) = (128usize, 96usize);
    let refp = plane(w, h, 3);
    let dst = plane(w, h, 4);
    let mut early_outs = 0usize;
    let mut full_runs = 0usize;

    for mat in legal_models(10) {
        for chess in [false, true] {
            for &best in &[i64::MAX, 1_000_000i64, 1000] {
                let wm0 = WarpedMotionParams {
                    wm_type: TransformationType::Affine,
                    wmmat: mat,
                    ..Default::default()
                };
                let mut c_io = wm_io(&wm0);
                let c_err = cref::warp_error(
                    &mut c_io, &refp, w, h, w, &dst, 0, 0, 0, w as i32, h as i32, w, 0, 0, chess,
                    best,
                );

                let mut r_wm = wm0;
                let r_err = gm::av1_warp_error(
                    &mut r_wm, &refp, w as i32, h as i32, w, &dst, 0, 0, 0, w as i32, h as i32, w,
                    0, 0, chess, best,
                );
                assert_eq!(
                    r_err, c_err,
                    "warp_error mismatch: mat {mat:?} chess {chess} best {best}"
                );
                assert_wm_eq(&r_wm, &c_io, "warp_error");
                if c_err > best {
                    early_outs += 1;
                } else {
                    full_runs += 1;
                }
            }
        }
    }
    assert!(
        early_outs > 5,
        "the early-out never fired ({early_outs}) — its un-doubled return is untested"
    );
    assert!(full_runs > 5, "no full run completed ({full_runs})");
}

/// A model the shear gate REJECTS must return the sentinel 1 from both sides.
#[test]
fn warp_error_returns_the_sentinel_on_an_illegal_model() {
    let (w, h) = (64usize, 64usize);
    let refp = plane(w, h, 5);
    let dst = plane(w, h, 6);
    // mat[2] <= 0 fails is_affine_valid.
    let mat = [0, 0, -PREC, 0, 0, PREC];
    assert!(!cref::get_shear_params(&mat).0);

    let wm0 = WarpedMotionParams {
        wm_type: TransformationType::Affine,
        wmmat: mat,
        ..Default::default()
    };
    let mut c_io = wm_io(&wm0);
    let c_err = cref::warp_error(
        &mut c_io,
        &refp,
        w,
        h,
        w,
        &dst,
        0,
        0,
        0,
        w as i32,
        h as i32,
        w,
        0,
        0,
        false,
        i64::MAX,
    );
    let mut r_wm = wm0;
    let r_err = gm::av1_warp_error(
        &mut r_wm,
        &refp,
        w as i32,
        h as i32,
        w,
        &dst,
        0,
        0,
        0,
        w as i32,
        h as i32,
        w,
        0,
        0,
        false,
        i64::MAX,
    );
    assert_eq!(c_err, 1, "C's illegal-model sentinel is 1");
    assert_eq!(r_err, c_err);
}

// ---------------------------------------------------------------------------
// 4. svt_av1_refine_integerized_param
//    (covers add_param_offset + force_wmtype + get_wmtype + warp_error)
// ---------------------------------------------------------------------------

#[test]
fn refine_integerized_param_matches_c() {
    let (w, h) = (128usize, 96usize);
    let refp = plane(w, h, 11);
    let dst = plane(w, h, 12);
    let mut moved = 0usize;
    let mut early = 0usize;

    for mat in legal_models(6) {
        for wmtype in [
            TransformationType::Translation,
            TransformationType::RotZoom,
            TransformationType::Affine,
        ] {
            for &n_ref in &[1i32, 3] {
                for chess in [false, true] {
                    for &(rfn_early_exit, pic_sad, params_cost) in
                        &[(false, 1_000_000u32, 100i32), (true, 1_000u32, 100)]
                    {
                        let wm0 = WarpedMotionParams {
                            wm_type: wmtype,
                            wmmat: mat,
                            ..Default::default()
                        };
                        let mut c_io = wm_io(&wm0);
                        let c_err = cref::refine_integerized_param(
                            rfn_early_exit,
                            &mut c_io,
                            wmtype as i32,
                            &refp,
                            w,
                            h,
                            w,
                            &dst,
                            w as i32,
                            h as i32,
                            w,
                            n_ref,
                            chess,
                            i64::MAX,
                            pic_sad,
                            params_cost,
                        );

                        let mut r_wm = wm0;
                        let r_err = gm::refine_integerized_param(
                            &GmRefineCtrls { rfn_early_exit },
                            &mut r_wm,
                            wmtype,
                            &refp,
                            w as i32,
                            h as i32,
                            w,
                            &dst,
                            w as i32,
                            h as i32,
                            w,
                            n_ref,
                            chess,
                            i64::MAX,
                            pic_sad,
                            params_cost,
                        );
                        assert_eq!(
                            r_err, c_err,
                            "refine error mismatch: mat {mat:?} {wmtype:?} n {n_ref} chess {chess} \
                             early {rfn_early_exit}"
                        );
                        assert_wm_eq(&r_wm, &c_io, "refine");
                        if r_wm.wmmat != mat {
                            moved += 1;
                        }
                        // The early-exit arm leaves wm as force_wmtype left it
                        // and skips the trailing get_wmtype; the C side is the
                        // oracle for that too, so just count that it fired.
                        if rfn_early_exit {
                            early += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        moved > 10,
        "the hill-climb never changed a parameter ({moved}) — add_param_offset is untested"
    );
    assert!(early > 0);
}

/// IDENTITY searches ZERO parameters (`max_trans_model_params[IDENTITY] == 0`),
/// so the whole climb is skipped and only the two `force_wmtype` calls plus the
/// final `get_wmtype` run. That is exactly the path a wrong fallthrough breaks.
#[test]
fn refine_integerized_param_identity_is_the_force_wmtype_path() {
    let (w, h) = (64usize, 64usize);
    let refp = plane(w, h, 21);
    let dst = plane(w, h, 22);
    let wm0 = WarpedMotionParams {
        wm_type: TransformationType::Affine,
        wmmat: [1234, -567, PREC + 900, 700, -700, PREC + 900],
        ..Default::default()
    };
    let mut c_io = wm_io(&wm0);
    let c_err = cref::refine_integerized_param(
        false,
        &mut c_io,
        TransformationType::Identity as i32,
        &refp,
        w,
        h,
        w,
        &dst,
        w as i32,
        h as i32,
        w,
        3,
        false,
        i64::MAX,
        1_000_000,
        100,
    );
    let mut r_wm = wm0;
    let r_err = gm::refine_integerized_param(
        &GmRefineCtrls {
            rfn_early_exit: false,
        },
        &mut r_wm,
        TransformationType::Identity,
        &refp,
        w as i32,
        h as i32,
        w,
        &dst,
        w as i32,
        h as i32,
        w,
        3,
        false,
        i64::MAX,
        1_000_000,
        100,
    );
    assert_eq!(r_err, c_err);
    assert_wm_eq(&r_wm, &c_io, "identity refine");
    // And the model really was forced all the way down.
    assert_eq!(ty_from_i32(c_io[0]), TransformationType::Identity);
}
