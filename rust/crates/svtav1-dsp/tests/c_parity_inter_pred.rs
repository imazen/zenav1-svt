//! Differential parity: sub-pel convolution vs the C reference
//! `svt_aom_convolve8_horiz_c` / `svt_aom_convolve8_vert_c` (convolve.c).
//!
//! SCOPE / HONEST MAPPING (audit 2026-07-14):
//!   * `convolve_horiz` / `convolve_vert` are single-pass 8-tap kernels with
//!     `clip_pixel(ROUND_POWER_OF_TWO(sum, 7))` — this is EXACTLY
//!     `svt_aom_convolve8_{horiz,vert}_c` at `x_step_q4 = 16`. These are the
//!     kernels `svt_aom_upsampled_pred_c` uses for ME sub-pel refinement.
//!     They are verified BIT-EXACT here over all 16 phases.
//!   * `convolve_2d` composes the two passes through a `u8` intermediate. That
//!     matches the `svt_aom_upsampled_pred_c` 2-D path, and is verified against
//!     the C kernels composed the same way. It is NOT the mainstream inter
//!     *reconstruction* convolve `svt_av1_convolve_2d_sr_c` (16-bit
//!     intermediate + ROUND0/ROUND1 offset scheme), which is not ported in
//!     this module — see the audit report. convolve.c did not change 4.1->4.2.

use svtav1_cref as cref;
use svtav1_dsp::inter_pred::{convolve_2d, convolve_horiz, convolve_vert};
use svtav1_tables::interp::SUB_PEL_FILTERS_8;

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
    fn byte(&mut self) -> u8 {
        (self.next() >> 33) as u8
    }
}

const SIZES: &[(usize, usize)] = &[
    (4, 4),
    (8, 8),
    (8, 16),
    (16, 8),
    (16, 16),
    (32, 8),
    (32, 32),
    (64, 16),
];

#[test]
fn convolve_horiz_matches_c_all_phases() {
    let mut rng = Rng(0xC0FFEE_11);
    for &(w, h) in SIZES {
        let stride = w + 8;
        let buf: Vec<u8> = (0..stride * h).map(|_| rng.byte()).collect();
        for phase in 0..16usize {
            let filter = &SUB_PEL_FILTERS_8[phase];
            let mut dst_rust = vec![0u8; w * h];
            convolve_horiz(&buf, stride, &mut dst_rust, w, filter, w, h);

            let mut dst_c = vec![0u8; w * h];
            // C origin = Rust base + 3 (kernel subtracts SUBPEL_TAPS/2-1 = 3).
            cref::convolve8_horiz(&buf, 3, stride, &mut dst_c, w, filter, w, h);
            assert_eq!(dst_rust, dst_c, "convolve_horiz {w}x{h} phase {phase}");
        }
    }
}

#[test]
fn convolve_vert_matches_c_all_phases() {
    let mut rng = Rng(0xC0FFEE_22);
    for &(w, h) in SIZES {
        let stride = w + 8;
        let buf: Vec<u8> = (0..stride * (h + 8)).map(|_| rng.byte()).collect();
        for phase in 0..16usize {
            let filter = &SUB_PEL_FILTERS_8[phase];
            let mut dst_rust = vec![0u8; w * h];
            convolve_vert(&buf, stride, &mut dst_rust, w, filter, w, h);

            let mut dst_c = vec![0u8; w * h];
            // C origin = Rust base + 3 rows.
            cref::convolve8_vert(&buf, 3 * stride, stride, &mut dst_c, w, filter, w, h);
            assert_eq!(dst_rust, dst_c, "convolve_vert {w}x{h} phase {phase}");
        }
    }
}

/// `convolve_2d` == C `convolve8_horiz` into a u8 intermediate, then C
/// `convolve8_vert` — i.e. the u8-intermediate 2-pass composition, bit-exact.
#[test]
fn convolve_2d_matches_c_two_pass_composition() {
    let mut rng = Rng(0xC0FFEE_33);
    for &(w, h) in SIZES {
        let src_stride = w + 8;
        let src_rows = h + 7;
        let src: Vec<u8> = (0..src_stride * src_rows).map(|_| rng.byte()).collect();

        for &(hp, vp) in &[(0usize, 0usize), (8, 8), (4, 12), (11, 3), (15, 6), (2, 14)] {
            let hf = &SUB_PEL_FILTERS_8[hp];
            let vf = &SUB_PEL_FILTERS_8[vp];

            let mut dst_rust = vec![0u8; w * h];
            convolve_2d(&src, src_stride, &mut dst_rust, w, hf, vf, w, h);

            // Mirror convolve_2d_inner via the C kernels.
            let inter_h = h + 7;
            let mut inter = vec![0u8; inter_h * w];
            cref::convolve8_horiz(&src, 3, src_stride, &mut inter, w, hf, w, inter_h);
            let mut dst_c = vec![0u8; w * h];
            cref::convolve8_vert(&inter, 3 * w, w, &mut dst_c, w, vf, w, h);

            assert_eq!(dst_rust, dst_c, "convolve_2d {w}x{h} hp={hp} vp={vp}");
        }
    }
}
