//! Differential (evidence tier 1): the whole `port_warp` chain against the REAL
//! exported symbols of `Codec/warped_motion.c`, driven through `svtav1-cref`.
//!
//! What is driven, and what each call transitively covers:
//!
//! | C symbol                     | port fn                  | also covers (static in C) |
//! |------------------------------|--------------------------|---------------------------|
//! | `svt_aom_warped_filter`      | `tables::WARPED_FILTER`  | — (table pin)             |
//! | `svt_get_shear_params`       | `get_shear_params`       | `is_affine_valid`, `is_affine_shear_allowed`, `resolve_divisor_32` |
//! | `svt_find_projection`        | `find_projection`        | `find_affine_int`, `resolve_divisor_64`, `get_mult_shift_diag/ndiag` |
//! | `svt_aom_select_samples`     | `select_samples`         | —                         |
//! | `svt_av1_warp_affine_c`      | `warp_affine`            | —                         |
//! | `svt_warp_plane`             | `warp_plane`             | —                         |
//! | `svt_av1_warp_plane`         | `av1_warp_plane`         | —                         |
//! | `svt_av1_highbd_warp_affine_c` | `highbd_warp_affine`   | —                         |
//!
//! There is no hand-derived vector anywhere in this file: every expectation is
//! a value the C library produced in this process.

use svtav1_cref as cref;
use svtav1_dsp::port_warp::{
    self, WARPEDMODEL_PREC_BITS, WarpConvolveParams, WarpPlaneIo, get_shear_params, tables,
};
use svtav1_types::block::BlockSize;
use svtav1_types::motion::{Mv, TransformationType, WarpedMotionParams};

const PREC: i32 = 1 << WARPEDMODEL_PREC_BITS;

/// `svtav1-cref` takes the shear as a tuple; the port and the shim return it
/// as an array. One conversion, spelled once.
fn tup(s: [i16; 4]) -> (i16, i16, i16, i16) {
    (s[0], s[1], s[2], s[3])
}

/// Deterministic pseudo-random byte source — no external crate, and the same
/// sequence on every host so a failure reproduces.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }
    fn next_u32(&mut self) -> u32 {
        // splitmix64
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

// ---------------------------------------------------------------------------
// 1. The constant table
// ---------------------------------------------------------------------------

/// The port's `WARPED_FILTER` is TRANSCRIBED from C; pin every one of its 193
/// rows against the real `svt_aom_warped_filter` array. The C file also
/// contains an `#elif WARPEDPIXEL_PREC_BITS == 5` arm of the same initializer
/// — reading that one yields a table of the wrong shape, so this test is what
/// proves the right arm was taken.
#[test]
fn warped_filter_table_matches_c_exactly() {
    for phase in 0..193 {
        let c = cref::warped_filter_row(phase as i32);
        assert_eq!(
            tables::WARPED_FILTER[phase],
            c,
            "svt_aom_warped_filter[{phase}] mismatch"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. svt_get_shear_params (covers is_affine_valid, is_affine_shear_allowed,
//    resolve_divisor_32)
// ---------------------------------------------------------------------------

fn shear_case(mat: [i32; 6]) {
    let (c_ok, c_shear) = cref::get_shear_params(&mat);
    let mut wm = WarpedMotionParams {
        wmmat: mat,
        ..Default::default()
    };
    let rust_ok = get_shear_params(&mut wm);
    assert_eq!(rust_ok, c_ok, "shear allowed mismatch for mat {mat:?}");
    if c_ok {
        assert_eq!(
            [wm.alpha, wm.beta, wm.gamma, wm.delta],
            c_shear,
            "shear params mismatch for mat {mat:?}"
        );
    } else {
        // C writes the shear fields BEFORE it can decide the model is illegal
        // (it only returns 0 after the WARP_PARAM bound check), so an
        // "illegal" verdict still leaves derived values behind. Those must
        // match too, since callers of the global-motion legality gate read
        // them regardless.
        if mat[2] > 0 {
            assert_eq!(
                [wm.alpha, wm.beta, wm.gamma, wm.delta],
                c_shear,
                "shear params (rejected model) mismatch for mat {mat:?}"
            );
        }
    }
}

#[test]
fn shear_params_match_c_on_structured_models() {
    // Identity.
    shear_case([0, 0, PREC, 0, 0, PREC]);
    // Pure zoom in / out.
    shear_case([0, 0, PREC + 4096, 0, 0, PREC + 4096]);
    shear_case([0, 0, PREC - 4096, 0, 0, PREC - 4096]);
    // Rotzoom-shaped: mat[5] = mat[2], mat[4] = -mat[3].
    shear_case([1234, -567, PREC + 900, 700, -700, PREC + 900]);
    // Illegal diagonal (mat[2] <= 0) -> is_affine_valid false.
    shear_case([0, 0, 0, 0, 0, PREC]);
    shear_case([0, 0, -PREC, 0, 0, PREC]);
    // Shear far outside the WARP_PARAM bound -> is_affine_shear_allowed false.
    shear_case([0, 0, PREC, 40000, -40000, PREC]);
}

#[test]
fn shear_params_match_c_on_random_models() {
    let mut rng = Rng::new(0xB0A7);
    let mut allowed = 0usize;
    let mut rejected = 0usize;
    for iter in 0..4000 {
        // Two regimes, deliberately. The WARP_PARAM bound is
        // `4|alpha| + 7|beta| < 1<<16`, so a model drawn uniformly over the
        // whole i32-ish range is rejected essentially always and the "allowed"
        // arm never fires. Half the draws are therefore near-identity (where
        // most are legal) and half are wide (where most are not), which is
        // what makes the anti-vacuity assertions below meaningful.
        let spread = if iter % 2 == 0 { 10_000 } else { PREC / 4 };
        let mat = [
            rng.range(-(1 << 20), 1 << 20),
            rng.range(-(1 << 20), 1 << 20),
            PREC + rng.range(-spread, spread),
            rng.range(-spread, spread),
            rng.range(-spread, spread),
            PREC + rng.range(-spread, spread),
        ];
        let (c_ok, c_shear) = cref::get_shear_params(&mat);
        let mut wm = WarpedMotionParams {
            wmmat: mat,
            ..Default::default()
        };
        let rust_ok = get_shear_params(&mut wm);
        assert_eq!(rust_ok, c_ok, "shear allowed mismatch for mat {mat:?}");
        assert_eq!(
            [wm.alpha, wm.beta, wm.gamma, wm.delta],
            c_shear,
            "shear params mismatch for mat {mat:?}"
        );
        if c_ok {
            allowed += 1;
        } else {
            rejected += 1;
        }
    }
    // Anti-vacuity: both verdicts were actually produced, so the test is not
    // silently exercising one arm.
    assert!(allowed > 100, "only {allowed} models were allowed");
    assert!(rejected > 100, "only {rejected} models were rejected");
}

// ---------------------------------------------------------------------------
// 3. svt_find_projection (covers find_affine_int, resolve_divisor_64,
//    get_mult_shift_diag / get_mult_shift_ndiag)
// ---------------------------------------------------------------------------

const BSIZES: [(BlockSize, i32); 6] = [
    (BlockSize::Block8x8, 3),
    (BlockSize::Block16x16, 6),
    (BlockSize::Block32x32, 9),
    (BlockSize::Block64x64, 12),
    (BlockSize::Block16x8, 5),
    (BlockSize::Block8x32, 18),
];

#[test]
fn find_projection_matches_c_on_random_sample_sets() {
    let mut rng = Rng::new(0x5EED_1234);
    let mut ok = 0usize;
    let mut failed = 0usize;

    for iter in 0..3000 {
        let (bsize, c_bsize) = BSIZES[iter % BSIZES.len()];
        let np = 1 + (rng.next_u32() % 8) as usize; // 1..=8 = LEAST_SQUARES_SAMPLES_MAX
        let mi_row = rng.range(0, 60);
        let mi_col = rng.range(0, 60);
        let mv = Mv {
            x: rng.range(-128, 128) as i16,
            y: rng.range(-128, 128) as i16,
        };
        // pts are 1/8-pel positions inside the frame; pts_inref are those
        // positions displaced by roughly the block MV plus noise, which is
        // exactly the shape adaptive_mv_pred.c builds.
        let mut pts1 = vec![0i32; np * 2];
        let mut pts2 = vec![0i32; np * 2];
        for i in 0..np {
            let x = (mi_col * 4 + rng.range(0, 63)) * 8;
            let y = (mi_row * 4 + rng.range(0, 63)) * 8;
            pts1[i * 2] = x;
            pts1[i * 2 + 1] = y;
            pts2[i * 2] = x + i32::from(mv.x) + rng.range(-24, 24);
            pts2[i * 2 + 1] = y + i32::from(mv.y) + rng.range(-24, 24);
        }

        let (c_failed, c_mat, c_shear) =
            cref::find_projection(&pts1, &pts2, c_bsize, (mv.x, mv.y), mi_row, mi_col);

        let mut wm = WarpedMotionParams {
            wmmat: [0; 6],
            alpha: 0,
            beta: 0,
            gamma: 0,
            delta: 0,
            ..Default::default()
        };
        let rust_failed =
            port_warp::find_projection(np, &pts1, &pts2, bsize, mv, &mut wm, mi_row, mi_col);

        assert_eq!(
            rust_failed, c_failed,
            "find_projection verdict mismatch (iter {iter}, np {np}, bsize {c_bsize})"
        );
        // C writes into the wm it was handed regardless of the verdict, so the
        // model is compared either way.
        assert_eq!(
            wm.wmmat, c_mat,
            "find_projection wmmat mismatch (iter {iter}, np {np}, bsize {c_bsize}, \
             mv {mv:?}, pts1 {pts1:?}, pts2 {pts2:?})"
        );
        assert_eq!(
            [wm.alpha, wm.beta, wm.gamma, wm.delta],
            c_shear,
            "find_projection shear mismatch (iter {iter})"
        );
        if c_failed {
            failed += 1;
        } else {
            ok += 1;
        }
    }
    assert!(ok > 50, "only {ok} projections succeeded");
    assert!(failed > 50, "only {failed} projections failed");
}

/// The degenerate cases C short-circuits on: a single sample (det == 0) and
/// collinear samples.
#[test]
fn find_projection_matches_c_on_degenerate_sets() {
    for (np, pts1, pts2) in [
        (1usize, vec![64, 64], vec![72, 72]),
        (2, vec![64, 64, 64, 64], vec![72, 72, 72, 72]),
        // Collinear along x -> A[1][1] contribution only from LS_STEP terms.
        (3, vec![0, 0, 8, 0, 16, 0], vec![8, 0, 16, 0, 24, 0]),
    ] {
        let mv = Mv { x: 8, y: 8 };
        let (c_failed, c_mat, c_shear) = cref::find_projection(&pts1, &pts2, 6, (mv.x, mv.y), 0, 0);
        let mut wm = WarpedMotionParams {
            wmmat: [0; 6],
            ..Default::default()
        };
        let rust_failed =
            port_warp::find_projection(np, &pts1, &pts2, BlockSize::Block16x16, mv, &mut wm, 0, 0);
        assert_eq!(rust_failed, c_failed, "degenerate np={np} verdict");
        assert_eq!(wm.wmmat, c_mat, "degenerate np={np} wmmat");
        assert_eq!(
            [wm.alpha, wm.beta, wm.gamma, wm.delta],
            c_shear,
            "degenerate np={np} shear"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. svt_aom_select_samples
// ---------------------------------------------------------------------------

#[test]
fn select_samples_matches_c_including_the_compaction() {
    let mut rng = Rng::new(0xDEAD_BEEF);
    let mut kept_all = 0usize;
    let mut kept_some = 0usize;
    for iter in 0..2000 {
        let (bsize, c_bsize) = BSIZES[iter % BSIZES.len()];
        let len = 1 + (rng.next_u32() % 8) as usize;
        let mv = Mv {
            x: rng.range(-64, 64) as i16,
            y: rng.range(-64, 64) as i16,
        };
        let mut pts = vec![0i32; len * 2];
        let mut inref = vec![0i32; len * 2];
        for i in 0..len {
            let x = rng.range(0, 512);
            let y = rng.range(0, 512);
            pts[i * 2] = x;
            pts[i * 2 + 1] = y;
            // Half the samples land inside the threshold, half well outside,
            // so both the keep and the drop path fire.
            let noise = if i % 2 == 0 {
                rng.range(-8, 8)
            } else {
                rng.range(-400, 400)
            };
            inref[i * 2] = x + i32::from(mv.x) + noise;
            inref[i * 2 + 1] = y + i32::from(mv.y) + noise;
        }

        let (mut c_pts, mut c_inref) = (pts.clone(), inref.clone());
        let c_n = cref::select_samples((mv.x, mv.y), &mut c_pts, &mut c_inref, len, c_bsize);

        let (mut r_pts, mut r_inref) = (pts.clone(), inref.clone());
        let r_n = port_warp::select_samples(mv, &mut r_pts, &mut r_inref, len, bsize);

        assert_eq!(r_n, c_n, "select_samples count mismatch (iter {iter})");
        // The compaction itself is what feeds find_projection, so compare the
        // arrays, not just the count. Only the first `n` pairs are meaningful.
        let n = c_n as usize;
        assert_eq!(
            r_pts[..n * 2],
            c_pts[..n * 2],
            "select_samples pts compaction mismatch (iter {iter})"
        );
        assert_eq!(
            r_inref[..n * 2],
            c_inref[..n * 2],
            "select_samples pts_inref compaction mismatch (iter {iter})"
        );
        if n == len {
            kept_all += 1;
        } else {
            kept_some += 1;
        }
    }
    assert!(kept_all > 10, "the keep-everything path never fired");
    assert!(kept_some > 10, "the drop-some path never fired");
}

// ---------------------------------------------------------------------------
// 5. svt_av1_warp_affine_c — the kernel itself
// ---------------------------------------------------------------------------

/// Trailing slack on every reference plane. `warp_plane` / `av1_warp_plane`
/// go through the RTCD `svt_av1_warp_affine`, i.e. the NEON kernel on this
/// host, which vector-loads a whole row before applying the `warp_pad_*`
/// edge replication and therefore reads past the last row of an
/// exactly-sized plane (measured 2026-08-31 as an intermittent SIGBUS in
/// `c_parity_global_motion`). A real reference frame always has borders; the
/// addressable `width` x `height` rectangle is unchanged.
const PLANE_SLACK: usize = 4096;

fn make_ref(w: usize, h: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut v: Vec<u8> = (0..w * h).map(|_| rng.range(0, 255) as u8).collect();
    v.resize(w * h + PLANE_SLACK, 0);
    v
}

/// A model that both C and the port agree is warp-legal, derived by asking C
/// for the shear and skipping the model when C rejects it.
fn legal_models() -> Vec<([i32; 6], [i16; 4])> {
    let mut rng = Rng::new(0xC0FFEE);
    let mut out = Vec::new();
    // Identity first — the trivial control.
    let ident = [0, 0, PREC, 0, 0, PREC];
    out.push((ident, cref::get_shear_params(&ident).1));
    while out.len() < 24 {
        let mat = [
            rng.range(-4 * PREC, 4 * PREC),
            rng.range(-4 * PREC, 4 * PREC),
            PREC + rng.range(-2000, 2000),
            rng.range(-2000, 2000),
            rng.range(-2000, 2000),
            PREC + rng.range(-2000, 2000),
        ];
        let (ok, shear) = cref::get_shear_params(&mat);
        if ok {
            out.push((mat, shear));
        }
    }
    out
}

#[test]
fn warp_affine_matches_c_non_compound() {
    let (w, h) = (64usize, 64usize);
    let refp = make_ref(w, h, 7);
    let mut nondegenerate = 0usize;

    for (mat, shear) in legal_models() {
        for &(p_col, p_row, p_w, p_h) in &[
            (8i32, 8i32, 8usize, 8usize),
            (0, 0, 16, 16),
            (16, 24, 8, 4), // p_height < 8 exercises the vertical AOMMIN crop
            (24, 16, 4, 8), // p_width  < 8 exercises the horizontal AOMMIN crop
            (0, 0, 32, 16),
        ] {
            let mut c_pred = vec![0u8; p_w * p_h];
            cref::warp_affine(
                &mat,
                &refp,
                w,
                h,
                w,
                &mut c_pred,
                p_col,
                p_row,
                p_w,
                p_h,
                p_w,
                tup(shear),
            );

            let mut r_pred = vec![0u8; p_w * p_h];
            let cp = WarpConvolveParams::simple(false, 8);
            port_warp::warp_affine(
                &mat,
                &refp,
                w as i32,
                h as i32,
                w,
                &mut r_pred,
                None,
                p_col,
                p_row,
                p_w as i32,
                p_h as i32,
                p_w,
                0,
                0,
                &cp,
                shear[0],
                shear[1],
                shear[2],
                shear[3],
            );
            assert_eq!(
                r_pred, c_pred,
                "warp_affine mismatch: mat {mat:?} shear {shear:?} block \
                 ({p_col},{p_row}) {p_w}x{p_h}"
            );
            if c_pred.iter().any(|&v| v != c_pred[0]) {
                nondegenerate += 1;
            }
        }
    }
    assert!(
        nondegenerate > 50,
        "only {nondegenerate} non-degenerate blocks — the oracle may be producing flat output"
    );
}

/// Chroma planes call the kernel with `subsampling_x/y = 1`; the projection
/// then happens in luma coordinates and is converted back. That geometry is as
/// normative as the filter, and `cref::warp_affine` hardwires 0/0, so this
/// uses the subsampling-aware shim.
#[test]
fn warp_affine_matches_c_with_chroma_subsampling() {
    let (w, h) = (48usize, 48usize);
    let refp = make_ref(w, h, 11);
    for (mat, shear) in legal_models().into_iter().take(8) {
        for &(sx, sy) in &[(1i32, 1i32), (1, 0), (0, 1)] {
            let (p_col, p_row, p_w, p_h) = (8i32, 8i32, 8usize, 8usize);
            let mut c_pred = vec![0u8; p_w * p_h];
            cref::warp_affine_sub(
                &mat,
                &refp,
                w,
                h,
                w,
                &mut c_pred,
                p_col,
                p_row,
                p_w,
                p_h,
                p_w,
                sx,
                sy,
                tup(shear),
            );
            let mut r_pred = vec![0u8; p_w * p_h];
            let cp = WarpConvolveParams::simple(false, 8);
            port_warp::warp_affine(
                &mat,
                &refp,
                w as i32,
                h as i32,
                w,
                &mut r_pred,
                None,
                p_col,
                p_row,
                p_w as i32,
                p_h as i32,
                p_w,
                sx,
                sy,
                &cp,
                shear[0],
                shear[1],
                shear[2],
                shear[3],
            );
            assert_eq!(
                r_pred, c_pred,
                "subsampled warp_affine mismatch: mat {mat:?} sub ({sx},{sy})"
            );
        }
    }
}

/// The compound arm: first pass writes the `ConvBufType` accumulator, second
/// pass averages it into the 8-bit prediction. Both distance-weighted and
/// plain averaging are exercised.
#[test]
fn warp_affine_matches_c_compound() {
    let (w, h) = (64usize, 64usize);
    let refp = make_ref(w, h, 23);
    let (p_col, p_row, p_w, p_h) = (8i32, 8i32, 8usize, 8usize);

    for (mat, shear) in legal_models().into_iter().take(10) {
        for jnt in [None, Some((5i32, 11i32))] {
            // --- pass 1: accumulate ---
            let mut c_dst = vec![0u16; p_w * p_h];
            let mut c_pred = vec![0u8; p_w * p_h];
            cref::warp_affine_compound(
                &mat,
                &refp,
                w,
                h,
                w,
                &mut c_pred,
                &mut c_dst,
                p_w,
                false,
                jnt,
                p_col,
                p_row,
                p_w,
                p_h,
                p_w,
                tup(shear),
            );

            let mut r_dst = vec![0u16; p_w * p_h];
            let mut r_pred = vec![0u8; p_w * p_h];
            let mut cp = WarpConvolveParams::no_round(false, p_w, true, 8);
            if let Some((f, b)) = jnt {
                cp.use_jnt_comp_avg = true;
                cp.fwd_offset = f;
                cp.bck_offset = b;
            }
            port_warp::warp_affine(
                &mat,
                &refp,
                w as i32,
                h as i32,
                w,
                &mut r_pred,
                Some(&mut r_dst),
                p_col,
                p_row,
                p_w as i32,
                p_h as i32,
                p_w,
                0,
                0,
                &cp,
                shear[0],
                shear[1],
                shear[2],
                shear[3],
            );
            assert_eq!(r_dst, c_dst, "compound accumulator mismatch, mat {mat:?}");
            assert!(
                c_dst.iter().any(|&v| v != 0),
                "compound accumulator is all zero — oracle not driven"
            );

            // --- pass 2: average a SECOND warp into the accumulator ---
            let mat2 = [mat[0] + 4096, mat[1] - 4096, mat[2], mat[3], mat[4], mat[5]];
            let (ok2, shear2) = cref::get_shear_params(&mat2);
            if !ok2 {
                continue;
            }
            cref::warp_affine_compound(
                &mat2,
                &refp,
                w,
                h,
                w,
                &mut c_pred,
                &mut c_dst,
                p_w,
                true,
                jnt,
                p_col,
                p_row,
                p_w,
                p_h,
                p_w,
                tup(shear2),
            );

            let mut cp2 = WarpConvolveParams::no_round(true, p_w, true, 8);
            if let Some((f, b)) = jnt {
                cp2.use_jnt_comp_avg = true;
                cp2.fwd_offset = f;
                cp2.bck_offset = b;
            }
            port_warp::warp_affine(
                &mat2,
                &refp,
                w as i32,
                h as i32,
                w,
                &mut r_pred,
                Some(&mut r_dst),
                p_col,
                p_row,
                p_w as i32,
                p_h as i32,
                p_w,
                0,
                0,
                &cp2,
                shear2[0],
                shear2[1],
                shear2[2],
                shear2[3],
            );
            assert_eq!(
                r_pred, c_pred,
                "compound averaged prediction mismatch, mat {mat:?}, jnt {jnt:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 6. svt_warp_plane / svt_av1_warp_plane — the drivers
// ---------------------------------------------------------------------------

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

/// Includes ROTZOOM, whose driver MUTATES the model (`wmmat[5] = wmmat[2]`,
/// `wmmat[4] = -wmmat[3]`) before predicting. The mutation is compared too.
#[test]
fn warp_plane_matches_c_including_the_rotzoom_fixup() {
    let (w, h) = (64usize, 64usize);
    let refp = make_ref(w, h, 31);
    let (p_col, p_row, p_w, p_h) = (8i32, 8i32, 16usize, 16usize);
    let mut saw_rotzoom_mutation = false;

    for (mat, shear) in legal_models().into_iter().take(12) {
        for wmtype in [
            TransformationType::Affine,
            TransformationType::RotZoom,
            TransformationType::Translation,
            TransformationType::Identity,
        ] {
            let wm0 = WarpedMotionParams {
                wm_type: wmtype,
                wmmat: mat,
                alpha: shear[0],
                beta: shear[1],
                gamma: shear[2],
                delta: shear[3],
                invalid: false,
            };

            let mut c_io = wm_io(&wm0);
            let mut c_pred = vec![0u8; p_w * p_h];
            cref::warp_plane(
                &mut c_io,
                &refp,
                w,
                h,
                w,
                &mut c_pred,
                p_col,
                p_row,
                p_w,
                p_h,
                p_w,
                0,
                0,
            );

            let mut r_wm = wm0;
            let mut r_pred = vec![0u8; p_w * p_h];
            let cp = WarpConvolveParams::simple(false, 8);
            port_warp::warp_plane(
                &mut r_wm,
                &refp,
                w as i32,
                h as i32,
                w,
                &mut r_pred,
                None,
                p_col,
                p_row,
                p_w as i32,
                p_h as i32,
                p_w,
                0,
                0,
                &cp,
            );

            assert_eq!(
                r_wm.wmmat,
                [c_io[1], c_io[2], c_io[3], c_io[4], c_io[5], c_io[6]],
                "warp_plane model fix-up mismatch for {wmtype:?}"
            );
            assert_eq!(r_pred, c_pred, "warp_plane pixels mismatch for {wmtype:?}");
            if wmtype == TransformationType::RotZoom && r_wm.wmmat != mat {
                saw_rotzoom_mutation = true;
            }
        }
    }
    assert!(
        saw_rotzoom_mutation,
        "the ROTZOOM fix-up never actually changed a model — the test would pass \
         even if the port omitted it"
    );
}

/// `svt_av1_warp_plane` with `use_hbd = 0` — the dispatcher entry point
/// `enc_inter_prediction.c` calls.
#[test]
fn av1_warp_plane_lowbd_matches_c() {
    let (w, h) = (64usize, 64usize);
    let refp = make_ref(w, h, 41);
    let (p_col, p_row, p_w, p_h) = (0i32, 0i32, 16usize, 16usize);
    for (mat, shear) in legal_models().into_iter().take(8) {
        let wm0 = WarpedMotionParams {
            wm_type: TransformationType::Affine,
            wmmat: mat,
            alpha: shear[0],
            beta: shear[1],
            gamma: shear[2],
            delta: shear[3],
            invalid: false,
        };
        let mut c_io = wm_io(&wm0);
        let mut c_pred = vec![0u8; p_w * p_h];
        cref::av1_warp_plane_lowbd(
            &mut c_io,
            &refp,
            w,
            h,
            w,
            &mut c_pred,
            p_col,
            p_row,
            p_w,
            p_h,
            p_w,
            0,
            0,
        );

        let mut r_wm = wm0;
        let mut r_pred = vec![0u8; p_w * p_h];
        let cp = WarpConvolveParams::simple(false, 8);
        port_warp::av1_warp_plane(
            &mut r_wm,
            WarpPlaneIo::Lowbd {
                reference: &refp,
                pred: &mut r_pred,
            },
            w as i32,
            h as i32,
            w,
            None,
            p_col,
            p_row,
            p_w as i32,
            p_h as i32,
            p_w,
            0,
            0,
            &cp,
        );
        assert_eq!(r_pred, c_pred, "av1_warp_plane (8-bit) mismatch");
    }
}

// ---------------------------------------------------------------------------
// 7. svt_av1_highbd_warp_affine_c
// ---------------------------------------------------------------------------

#[test]
fn highbd_warp_affine_matches_c() {
    // 10-bit ONLY, and that is a property of the C oracle, not a shortcut.
    // `svt_av1_highbd_warp_affine_c` reads its reference as SVT's 8+2 packed
    // pair — `(ref8b[..] << 2) | ((ref2b[..] >> 6) & 3)` — which carries
    // exactly TEN bits. There is no way to hand it a 12-bit sample through
    // that layout, so a "bd = 12" differential would be comparing the port's
    // 12-bit answer against C's truncated 10-bit one. (C ships 8/10-bit only
    // anyway — `svt_av1_verify_settings`, enc_settings.c:460.)
    let (w, h) = (64usize, 64usize);
    for bd in [10i32] {
        let mut rng = Rng::new(0x1010 + bd as u64);
        let maxv = (1i32 << bd) - 1;
        let refp: Vec<u16> = (0..w * h).map(|_| rng.range(0, maxv) as u16).collect();
        let mut nondegenerate = 0usize;

        for (mat, shear) in legal_models().into_iter().take(12) {
            for &(p_col, p_row, p_w, p_h) in
                &[(8i32, 8i32, 8usize, 8usize), (0, 0, 16, 16), (16, 24, 8, 4)]
            {
                let mut c_pred = vec![0u16; p_w * p_h];
                cref::highbd_warp_affine(
                    &mat,
                    &refp,
                    w,
                    h,
                    w,
                    &mut c_pred,
                    p_col,
                    p_row,
                    p_w,
                    p_h,
                    p_w,
                    0,
                    0,
                    bd,
                    tup(shear),
                );
                let mut r_pred = vec![0u16; p_w * p_h];
                let cp = WarpConvolveParams::simple(false, bd);
                port_warp::highbd_warp_affine(
                    &mat,
                    &refp,
                    w as i32,
                    h as i32,
                    w,
                    &mut r_pred,
                    None,
                    p_col,
                    p_row,
                    p_w as i32,
                    p_h as i32,
                    p_w,
                    0,
                    0,
                    bd,
                    &cp,
                    shear[0],
                    shear[1],
                    shear[2],
                    shear[3],
                );
                assert_eq!(
                    r_pred, c_pred,
                    "highbd warp_affine mismatch at bd {bd}: mat {mat:?}"
                );
                if c_pred.iter().any(|&v| v != c_pred[0]) {
                    nondegenerate += 1;
                }
            }
        }
        assert!(
            nondegenerate > 20,
            "bd {bd}: only {nondegenerate} non-degenerate blocks"
        );
    }
}
