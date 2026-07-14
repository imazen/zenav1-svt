//! Differential parity: OBMC blend vs the C reconstruction-side reference
//! (`build_obmc_inter_pred_{above,left}` in enc_inter_prediction.c, which call
//! `svt_aom_blend_a64_{v,h}mask_c` with `svt_av1_get_obmc_mask(overlap)`).
//!
//! OBMC blends the current block prediction (src0) with a neighbor prediction
//! (src1): `AOM_BLEND_A64(mask, cur, nb) = (mask*cur + (64-mask)*nb + 32) >> 6`.
//! The mask is the canonical `obmc_mask_{1,2,4,8,16,32}` table. inter_prediction.c
//! (masks) and blend_a64_mask.c (kernels) did not change 4.1->4.2.
//!
//! This is the one audited DSP module wired into the encoder (partition.rs,
//! inter path — dormant for the still gates). The port was STALE: wrong mask
//! values and inverted blend weighting; this suite pins the fix.

use svtav1_cref as cref;
use svtav1_dsp::obmc::{obmc_blend_above, obmc_blend_left};

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

/// All overlaps `svt_av1_get_obmc_mask` supports.
const OVERLAPS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[test]
fn obmc_blend_above_matches_c() {
    let mut rng = Rng(0x0B_11CE_01);
    for &overlap in OVERLAPS {
        for &w in &[4usize, 8, 16, 32, 64] {
            let h = overlap; // blend the full overlap band
            let dst_stride = w + 5;
            let above_stride = w + 3;
            let base: Vec<u8> = (0..dst_stride * h).map(|_| rng.byte()).collect();
            let above: Vec<u8> = (0..above_stride * h).map(|_| rng.byte()).collect();

            let mut dst_rust = base.clone();
            obmc_blend_above(&mut dst_rust, dst_stride, &above, above_stride, w, h, overlap);

            let mut dst_c = base.clone();
            cref::obmc_blend_above(&mut dst_c, dst_stride, &above, above_stride, w, overlap);

            assert_eq!(dst_rust, dst_c, "obmc above overlap={overlap} w={w}");
        }
    }
}

#[test]
fn obmc_blend_left_matches_c() {
    let mut rng = Rng(0x0B_11CE_02);
    for &overlap in OVERLAPS {
        for &h in &[4usize, 8, 16, 32, 64] {
            let w = overlap; // blend the full overlap band of columns
            let dst_stride = w + 5;
            let left_stride = w + 3;
            let base: Vec<u8> = (0..dst_stride * h).map(|_| rng.byte()).collect();
            let left: Vec<u8> = (0..left_stride * h).map(|_| rng.byte()).collect();

            let mut dst_rust = base.clone();
            obmc_blend_left(&mut dst_rust, dst_stride, &left, left_stride, w, h, overlap);

            let mut dst_c = base.clone();
            cref::obmc_blend_left(&mut dst_c, dst_stride, &left, left_stride, overlap, h);

            assert_eq!(dst_rust, dst_c, "obmc left overlap={overlap} h={h}");
        }
    }
}

/// The Rust `obmc_blend_above` blends only the top `overlap` rows of a taller
/// block; rows below the band must stay untouched. Also exercises wider blocks.
#[test]
fn obmc_blend_above_partial_band_matches_c() {
    let mut rng = Rng(0x0B_11CE_03);
    for &overlap in &[2usize, 4, 8] {
        let w = 32;
        let h = 32; // block taller than the overlap band
        let dst_stride = w + 7;
        let above_stride = w;
        let base: Vec<u8> = (0..dst_stride * h).map(|_| rng.byte()).collect();
        let above: Vec<u8> = (0..above_stride * overlap).map(|_| rng.byte()).collect();

        let mut dst_rust = base.clone();
        obmc_blend_above(&mut dst_rust, dst_stride, &above, above_stride, w, h, overlap);

        // C blends exactly the overlap band; rows >= overlap are unchanged.
        let mut dst_c = base.clone();
        cref::obmc_blend_above(&mut dst_c, dst_stride, &above, above_stride, w, overlap);

        assert_eq!(dst_rust, dst_c, "obmc above partial overlap={overlap}");
    }
}
