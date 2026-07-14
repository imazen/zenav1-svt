//! Differential audit: scaled inter prediction vs `svt_av1_convolve_2d_scale_c`
//! (inter_prediction.c). This C file's scale kernel did not change 4.1->4.2.
//!
//! FINDING — `scale.rs::scaled_prediction` is a STUB, not a port of the C
//! kernel. The C `svt_av1_convolve_2d_scale_c` uses the SCALE_SUBPEL_BITS=10
//! phase domain, `EIGHTTAP` `InterpFilterParams`, and a 2-stage ROUND0/ROUND1
//! convolution through a 16-bit intermediate. `scaled_prediction` uses a Q14
//! phase domain, a 4-bit phase into `SUB_PEL_FILTERS_8`, and a naive u8
//! intermediate `(sum+64)>>7` twice — a different algorithm with a different
//! API (it takes `ScaleFactors`/block coords, not filter params + step_qn).
//!
//! This suite (a) proves the C oracle is callable and behaves correctly on the
//! integer-scale identity case, and (b) PINS the divergence from the stub on a
//! fractional scale. A real port must rewrite scaled_prediction to the
//! convolve_2d_scale contract; then the `assert_ne!` becomes `assert_eq!`.

use svtav1_cref as cref;
use svtav1_dsp::scale::{scaled_prediction, ScaleFactors};

const SUBPEL: i32 = 1 << 10; // SCALE_SUBPEL_BITS

/// The C oracle at an exact 1:1 scale (step = 1<<10, subpel 0) with phase-0
/// filters is a straight copy — validates the harness/params.
#[test]
fn c_convolve_2d_scale_unity_is_copy() {
    let w = 8usize;
    let h = 8usize;
    let stride = 32usize;
    let src: Vec<u8> = (0..stride * 32).map(|i| ((i * 9 + 5) & 0xff) as u8).collect();
    let origin = 8 * stride + 8;
    let mut dst = vec![0u8; w * h];
    // step = 1<<10 (1:1), subpel 0 -> samples integer positions, phase 0 (identity).
    cref::convolve_2d_scale(&src, origin, stride, &mut dst, w, w, h, 0, SUBPEL, 0, SUBPEL);
    for r in 0..h {
        for c in 0..w {
            assert_eq!(
                dst[r * w + c],
                src[origin + r * stride + c],
                "unity-scale copy at ({r},{c})"
            );
        }
    }
}

/// GAP-PIN: on a fractional (1.5x) scale the Rust stub diverges from the C
/// oracle. Flip to `assert_eq!` when scale.rs is ported to convolve_2d_scale.
#[test]
fn scaled_prediction_diverges_from_c_on_fractional_scale() {
    let stride = 48usize;
    let plane: Vec<u8> = (0..stride * 48)
        .map(|i| ((i * 17 + (i / stride) * 3) & 0xff) as u8)
        .collect();

    // C: 1.5x step (1536 in the 10-bit domain), start subpel 0, interior origin.
    let w = 8usize;
    let h = 8usize;
    let step = 3 * SUBPEL / 2; // 1536
    let origin = 8 * stride + 8;
    let mut dst_c = vec![0u8; w * h];
    cref::convolve_2d_scale(&plane, origin, stride, &mut dst_c, w, w, h, 0, step, 0, step);
    assert!(dst_c.iter().any(|&v| v != dst_c[0]), "C scale non-degenerate");

    // Rust: nominal 1.5x downscale (x_scale = 1.5<<14) sampling from origin.
    let sf = ScaleFactors::new(48, 48, 32, 32); // ratio 1.5 -> x_scale=24576
    assert_eq!(sf.x_scale, 3 * (1 << 14) / 2);
    let mut dst_rust = vec![0u8; w * h];
    scaled_prediction(
        &plane, stride, &mut dst_rust, w, /*block_x*/ 8, /*block_y*/ 8, w, h, &sf, 48, 48,
    );

    assert_ne!(
        dst_rust, dst_c,
        "STUB pinned: scaled_prediction is not svt_av1_convolve_2d_scale_c \
         (see module doc). When ported, change this to assert_eq!"
    );
}
