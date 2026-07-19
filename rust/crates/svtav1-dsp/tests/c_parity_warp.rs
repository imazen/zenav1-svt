//! Differential audit: warped-motion prediction vs `svt_av1_warp_affine_c`
//! (warped_motion.c). warped_motion.c did not change 4.1->4.2.
//!
//! FINDING — `warp.rs::warp_prediction` is a STUB, not a port of the C kernel:
//!   * C samples `svt_aom_warped_filter` (WARPEDPIXEL_PREC_SHIFTS = 64 ->
//!     ~193 phases); Rust uses the 16-phase `SUB_PEL_FILTERS_8`.
//!   * C applies the alpha/beta/gamma/delta shear per 8x8 block; Rust ignores
//!     the shear entirely and maps each pixel independently.
//!   * C uses conv_params ROUND0/ROUND1 with an offset-bits scheme; Rust does a
//!     plain `(sum+64)>>7` twice.
//!   * Rust also multiplies `m0`/`m1` by `1<<16` although they are already Q16.
//!
//! This suite (a) proves the C oracle is callable/correct on the identity case
//! and (b) PINS the divergence on a sub-pel model. When warp.rs is properly
//! ported, the `assert_ne!` below must become `assert_eq!` and pass.

use svtav1_cref as cref;
use svtav1_dsp::warp::warp_prediction;
use svtav1_types::motion::WarpedMotionParams;

const PREC: i32 = 1 << 16; // WARPEDMODEL_PREC_BITS

/// The C oracle on an identity model (mat = [0,0,1<<16,0,0,1<<16], no shear)
/// reproduces SMOOTH reference content closely. It is NOT a bit-exact copy:
/// `svt_aom_warped_filter[64]` (the zero-phase center) is a real interpolation
/// filter with small lobes, and warp_affine adds a two-stage ROUND0/ROUND1
/// offset scheme — so identity warp is a mild low-pass (a genuine property of
/// the C kernel, not a bug; on high-frequency content it deviates by a few
/// LSBs). On a smooth ramp the deviation is <=2. Validates the harness/params.
#[test]
fn c_warp_affine_identity_near_copy_smooth() {
    let w = 24usize;
    let h = 24usize;
    // Smooth, non-wrapping ramp so a near-identity low-pass reproduces it.
    let refp: Vec<u8> = (0..w * h)
        .map(|i| (40 + (i / w) * 3 + (i % w) * 2) as u8)
        .collect();
    let mat = [0, 0, PREC, 0, 0, PREC];
    let (pw, ph) = (8usize, 8usize);
    let mut pred = vec![0u8; pw * ph];
    // Block at (p_col=8, p_row=8) — interior, no edge clamping.
    cref::warp_affine(&mat, &refp, w, h, w, &mut pred, 8, 8, pw, ph, pw, (0, 0, 0, 0));
    for r in 0..ph {
        for c in 0..pw {
            let got = pred[r * pw + c] as i32;
            let want = refp[(8 + r) * w + (8 + c)] as i32;
            assert!(
                (got - want).abs() <= 2,
                "identity warp near-copy at ({r},{c}): got {got} want {want}"
            );
        }
    }
    // And it is a real (non-degenerate) block.
    assert!(pred.iter().any(|&v| v != pred[0]));
}

/// GAP-PIN: on a sub-pel zoom model the Rust stub diverges from the C oracle.
/// Flip to `assert_eq!` when warp.rs is ported to svt_av1_warp_affine_c.
#[test]
fn warp_prediction_diverges_from_c_on_zoom() {
    let w = 32usize;
    let h = 32usize;
    let refp: Vec<u8> = (0..w * h)
        .map(|i| ((i * 13 + (i / w) * 5 + 3) & 0xff) as u8)
        .collect();
    // 0.5x zoom: mat[2]=mat[5]=0.5<<16 -> sub-pixel sampling every step.
    let half = PREC / 2;
    let mat = [0, 0, half, 0, 0, half];
    let (pw, ph) = (8usize, 8usize);

    let mut pred_c = vec![0u8; pw * ph];
    cref::warp_affine(&mat, &refp, w, h, w, &mut pred_c, 8, 8, pw, ph, pw, (0, 0, 0, 0));
    // Oracle produced a real (non-degenerate) block.
    assert!(pred_c.iter().any(|&v| v != pred_c[0]), "C warp non-degenerate");

    let params = WarpedMotionParams {
        wmmat: [0, 0, half, 0, 0, half],
        ..Default::default()
    };
    let mut pred_rust = vec![0u8; pw * ph];
    warp_prediction(&refp, w, &mut pred_rust, pw, &params, 8, 8, pw, ph, w, h);

    assert_ne!(
        pred_rust, pred_c,
        "STUB pinned: warp_prediction is not svt_av1_warp_affine_c (see module doc). \
         When ported, change this to assert_eq!"
    );
}
