//! Differential parity for `svtav1_dsp::subpel_variance` against the REAL
//! exported C symbols — **evidence tier 1** (`rust/docs/WORKING-ON-THIS.md` §4).
//!
//! Oracles, both `_c` spellings out of `libSvtAv1Enc.a`:
//! * `svt_aom_sub_pixel_variance{W}x{H}_c` — `AomVarianceFnPtr::svf`, the error
//!   metric the PRUNED sub-pixel tree minimises (`mcomp.c:156`).
//! * `svt_aom_variance{W}x{H}_c` — `::vf`, which `SUBPIX_VAR` calls last and
//!   which `svt_upsampled_setup_center_error` calls directly.
//!
//! All 22 instantiated sizes (`variance.c:208-229`) are driven, so the port's
//! runtime `(w, h)` parameterisation is checked against every macro expansion
//! rather than assumed to generalise from one.

use svtav1_cref::md_subpel as cref;
use svtav1_dsp::subpel_variance::{sub_pixel_variance, variance_diff_sse};

/// The 22 `VARIANCES(W, H)` instantiations, in source order.
const SIZES: [(usize, usize); 22] = [
    (128, 128),
    (128, 64),
    (64, 128),
    (64, 64),
    (64, 32),
    (32, 64),
    (32, 32),
    (32, 16),
    (16, 32),
    (16, 16),
    (16, 8),
    (8, 16),
    (8, 8),
    (8, 4),
    (4, 8),
    (4, 4),
    (4, 16),
    (16, 4),
    (8, 32),
    (32, 8),
    (16, 64),
    (64, 16),
];

struct Rng(u64);
impl Rng {
    fn next_u8(&mut self) -> u8 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 33) as u8
    }
}

#[test]
fn sub_pixel_variance_matches_c_all_sizes_all_phases() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for &(w, h) in &SIZES {
        // The kernel reads h+1 rows and w+1 columns, and the callers hand it a
        // strided plane, so pad both dimensions and use a stride wider than w.
        let a_stride = w + 13;
        let a: Vec<u8> = (0..a_stride * (h + 4)).map(|_| rng.next_u8()).collect();
        let b_stride = w + 5;
        let b: Vec<u8> = (0..b_stride * (h + 2)).map(|_| rng.next_u8()).collect();
        for xoffset in 0..8usize {
            for yoffset in 0..8usize {
                for &(a_base, b_base) in &[(0usize, 0usize), (a_stride + 3, b_stride + 1)] {
                    let (cvar, csse) = cref::sub_pixel_variance(
                        w,
                        h,
                        &a,
                        a_base,
                        a_stride,
                        xoffset as i32,
                        yoffset as i32,
                        &b,
                        b_base,
                        b_stride,
                    );
                    let (rvar, rsse) = sub_pixel_variance(
                        &a, a_base, a_stride, xoffset, yoffset, &b, b_base, b_stride, w, h,
                    );
                    assert_eq!(
                        (rvar, rsse),
                        (cvar, csse),
                        "sub_pixel_variance{w}x{h} phase ({xoffset},{yoffset}) bases ({a_base},{b_base})"
                    );
                }
            }
        }
    }
}

#[test]
fn variance_diff_sse_matches_c_all_sizes() {
    let mut rng = Rng(0x0fed_cba9_8765_4321);
    for &(w, h) in &SIZES {
        let a_stride = w + 7;
        let a: Vec<u8> = (0..a_stride * (h + 2)).map(|_| rng.next_u8()).collect();
        let b_stride = w + 3;
        let b: Vec<u8> = (0..b_stride * (h + 2)).map(|_| rng.next_u8()).collect();
        for &(a_base, b_base) in &[(0usize, 0usize), (a_stride + 2, b_stride + 1)] {
            let (cvar, csse) = cref::variance_vf(w, h, &a, a_base, a_stride, &b, b_base, b_stride);
            let (rvar, rsse) = variance_diff_sse(&a, a_base, a_stride, &b, b_base, b_stride, w, h);
            assert_eq!(
                (rvar, rsse),
                (cvar, csse),
                "variance{w}x{h} bases ({a_base},{b_base})"
            );
        }
    }
}

/// Extremes: an all-zero vs all-255 block maximises `sse` and drives the
/// `sse - sum^2/n` subtraction to exactly zero (constant difference), which a
/// port that returned plain `sse` would fail. Both blocks constant also makes
/// the bilinear passes exact so the phase index cannot hide a bug.
#[test]
fn sub_pixel_variance_extremes_match_c() {
    for &(w, h) in &SIZES {
        let a_stride = w + 1;
        let a = vec![255u8; a_stride * (h + 1)];
        let b = vec![0u8; w * h];
        for phase in 0..8usize {
            let (cvar, csse) = cref::sub_pixel_variance(
                w,
                h,
                &a,
                0,
                a_stride,
                phase as i32,
                phase as i32,
                &b,
                0,
                w,
            );
            let (rvar, rsse) = sub_pixel_variance(&a, 0, a_stride, phase, phase, &b, 0, w, w, h);
            assert_eq!((rvar, rsse), (cvar, csse), "extremes {w}x{h} phase {phase}");
            assert_eq!(rvar, 0, "constant difference must have zero variance");
        }
    }
}
