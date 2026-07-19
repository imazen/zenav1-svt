//! Differential parity: the SAD arithmetic inside `motion_est::full_pel_search`
//! vs the C reference `svt_aom_sad{w}x{h}_c` (compute_sad kernels).
//!
//! AUDIT 2026-07-14. `motion_est` is a HOMEGROWN full-pel + bilinear-subpel
//! searcher — NOT a port of C `svt_aom_motion_estimation_b64`
//! (motion_estimation.c) / the `mcomp.c` subpel tree / `av1me.c`, all of which
//! are bit-affecting-changed 4.1->4.2. It is non-normative (ME only picks
//! which MV/bits, never the bitstream format) and inter-only (partition.rs
//! inter path + the pipeline MV-map update, both gated on reference data that
//! is absent on key/still frames) — dormant for the conformance + identity
//! gates.
//!
//! What IS verifiable bit-exactly here is the one C-equivalent kernel the
//! searcher re-implements inline: the block SAD. This suite proves the
//! distortion `full_pel_search` reports at its chosen MV equals the C
//! `svt_aom_sad` of that exact block pair. The search *strategy* (raster scan,
//! early termination, tie-breaking) and the bilinear half/quarter-pel
//! refinement (vs C's 8-tap `svt_aom_upsampled_pred`) remain homegrown and are
//! not claimed to match C.

use svtav1_cref as cref;
use svtav1_encoder::motion_est::full_pel_search;
use svtav1_types::motion::Mv;

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

/// For random content the reported best distortion must equal the C SAD of the
/// block at the chosen MV (proves the inline SAD == `svt_aom_sad`).
#[test]
fn full_pel_distortion_equals_c_sad() {
    let mut rng = Rng(0x0713_051D);
    let (bw, bh) = (16usize, 16usize);
    let (pw, ph) = (64usize, 64usize);
    let (ox, oy) = (24i32, 20i32);

    for _ in 0..64 {
        let src: Vec<u8> = (0..bw * bh).map(|_| rng.byte()).collect();
        let refp: Vec<u8> = (0..pw * ph).map(|_| rng.byte()).collect();

        let res = full_pel_search(
            &src, bw, &refp, pw, ox, oy, bw, bh, Mv::ZERO, 6, 6, pw, ph,
        );

        // The full-pel offset the searcher settled on.
        let mvx_full = (res.mv.x as i32) / 8;
        let mvy_full = (res.mv.y as i32) / 8;
        let rx = ox + mvx_full;
        let ry = oy + mvy_full;
        assert!(rx >= 0 && ry >= 0 && (rx as usize + bw) <= pw && (ry as usize + bh) <= ph);
        let ref_origin = ry as usize * pw + rx as usize;

        let c_sad = cref::sad(bw, bh, &src, 0, bw, &refp, ref_origin, pw);
        assert_eq!(
            res.distortion, c_sad,
            "full_pel distortion {} != C sad {} at mv=({},{})",
            res.distortion, c_sad, res.mv.x, res.mv.y
        );
    }
}

/// Exact-match placement: distortion is zero and the MV points at the block,
/// and the C SAD there is likewise zero.
#[test]
fn full_pel_exact_match_zero_sad() {
    let (bw, bh) = (16usize, 16usize);
    let (pw, ph) = (64usize, 64usize);
    let mut rng = Rng(0xE_AC7);
    let src: Vec<u8> = (0..bw * bh).map(|_| rng.byte()).collect();

    // Plant the src block at (28, 22) in the reference; search around (24, 20).
    let (ox, oy) = (24i32, 20i32);
    let (tx, ty) = (28usize, 22usize);
    let mut refp = vec![0u8; pw * ph];
    for r in 0..bh {
        for c in 0..bw {
            refp[(ty + r) * pw + (tx + c)] = src[r * bw + c];
        }
    }

    let res = full_pel_search(
        &src, bw, &refp, pw, ox, oy, bw, bh, Mv::ZERO, 8, 8, pw, ph,
    );
    assert_eq!(res.distortion, 0, "exact match should be zero SAD");
    assert_eq!(res.mv.x, ((tx as i32 - ox) * 8) as i16);
    assert_eq!(res.mv.y, ((ty as i32 - oy) * 8) as i16);

    let ref_origin = ty * pw + tx;
    assert_eq!(cref::sad(bw, bh, &src, 0, bw, &refp, ref_origin, pw), 0);
}
