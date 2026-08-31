//! Differential (evidence tier 1): the high-bit-depth resize ladder against
//! the REAL exported symbols of `Codec/resize.c`.
//!
//! | C symbol                                  | port fn                        | also covers (static in C) |
//! |-------------------------------------------|--------------------------------|---------------------------|
//! | `svt_av1_highbd_interpolate_core_c`       | `highbd_interpolate_core`      | —                         |
//! | `svt_av1_highbd_down2_symeven_c`          | `highbd_down2_symeven`         | —                         |
//! | `svt_av1_highbd_resize_plane_horizontal`  | `highbd_resize_plane_horizontal`| `highbd_resize_multistep`, `highbd_interpolate`, `highbd_down2_symodd` |
//!
//! This closes the `docs/REFUSED-CONFIGS.md` CAPABILITY entry "superres is
//! 8-bit only so far (the u16 source downscale is unported)".

use svtav1_cref as cref;
use svtav1_dsp::port_resize_hbd::{
    highbd_down2_symeven, highbd_down2_symodd, highbd_interpolate_core, highbd_resize_multistep,
    highbd_resize_plane_horizontal,
};
use svtav1_dsp::resize::{SUBPEL_TAPS, choose_interp_filter};

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

fn line(len: usize, seed: u64, bd: i32) -> Vec<u16> {
    let mut rng = Rng::new(seed);
    let maxv = (1i32 << bd) - 1;
    (0..len).map(|_| rng.range(0, maxv) as u16).collect()
}

/// Every filter bank x a spread of length ratios, at both shipping bit depths.
/// The short-input regime (`x1 > x2`, where C takes its single fully-clamped
/// loop instead of the three-part split) is reached by the tiny lengths.
#[test]
fn highbd_interpolate_core_matches_c() {
    for bd in [10i32, 12] {
        for bank in 0..5i32 {
            for &(inl, outl) in &[
                (8usize, 6usize),
                (9, 5),
                (16, 9),
                (64, 33),
                (64, 64),
                (33, 64),
                (5, 9),
                (2, 7),
                (7, 2),
                (128, 72),
            ] {
                let input = line(inl, (bank as u64) * 17 + inl as u64, bd);
                let mut c_out = vec![0u16; outl];
                cref::highbd_interpolate_core(&input, inl, &mut c_out, outl, bd, bank);

                // Pick the same bank on the port side by index, so this tests
                // the KERNEL rather than choose_interp_filter (which the 8-bit
                // suite already gates).
                let filters: &[[i16; SUBPEL_TAPS]; 64] = match bank {
                    1 => &svtav1_dsp::resize::FILTEREDINTERP_FILTERS875,
                    2 => &svtav1_dsp::resize::FILTEREDINTERP_FILTERS750,
                    3 => &svtav1_dsp::resize::FILTEREDINTERP_FILTERS625,
                    4 => &svtav1_dsp::resize::FILTEREDINTERP_FILTERS500,
                    _ => choose_interp_filter(1, 1), // the normative table
                };
                let mut r_out = vec![0u16; outl];
                highbd_interpolate_core(&input, inl, &mut r_out, outl, bd, filters);
                assert_eq!(
                    r_out, c_out,
                    "highbd interpolate mismatch bd {bd} bank {bank} {inl}->{outl}"
                );
            }
        }
    }
}

#[test]
fn highbd_down2_symeven_matches_c() {
    for bd in [10i32, 12] {
        for &len in &[2usize, 4, 8, 16, 64, 128, 6, 10, 34] {
            let input = line(len, len as u64 * 3 + bd as u64, bd);
            let mut c_out = vec![0u16; len.div_ceil(2)];
            cref::highbd_down2_symeven(&input, len, &mut c_out, bd);
            let mut r_out = vec![0u16; len.div_ceil(2)];
            highbd_down2_symeven(&input, len, &mut r_out, bd);
            assert_eq!(
                r_out, c_out,
                "highbd down2_symeven mismatch bd {bd} len {len}"
            );
        }
    }
}

/// The whole ladder. `svt_av1_highbd_resize_plane_horizontal` drives the three
/// C statics, so this is what covers `highbd_down2_symodd` (reached when the
/// current filtered length is odd) and `highbd_resize_multistep`'s step
/// bookkeeping.
#[test]
fn highbd_resize_plane_horizontal_matches_c_over_the_superres_denominators() {
    for bd in [10i32, 12] {
        // The superres source downscale is width * 8 / denom for denom 9..16.
        for denom in 9..=16usize {
            for &(w, h) in &[(64usize, 8usize), (128, 4), (96, 6), (65, 3), (200, 2)] {
                let w2 = (w * 8).div_ceil(denom);
                let in_stride = w + 7;
                let out_stride = w2 + 5;
                let input = line((h - 1) * in_stride + w, (denom * 31 + w) as u64, bd);

                let mut c_out = vec![0u16; (h - 1) * out_stride + w2];
                cref::highbd_resize_plane_horizontal(
                    &input, h, w, in_stride, &mut c_out, w2, out_stride, bd,
                );
                let mut r_out = vec![0u16; (h - 1) * out_stride + w2];
                highbd_resize_plane_horizontal(
                    &input, h, w, in_stride, &mut r_out, w2, out_stride, bd,
                );
                assert_eq!(
                    r_out, c_out,
                    "highbd resize mismatch bd {bd} denom {denom} {w}x{h} -> {w2}"
                );
                assert!(
                    c_out.iter().any(|&v| v != c_out[0]),
                    "oracle produced a flat plane at denom {denom}"
                );
            }
        }
    }
}

/// `highbd_down2_symodd` is `static` and only reachable when an intermediate
/// length is ODD. Drive it through the exported plane function on odd widths
/// with a denominator that takes the 2:1 step, and pin that the odd arm really
/// fired by checking it against the port's own symodd on the same line.
#[test]
fn highbd_down2_symodd_is_reached_and_matches() {
    let bd = 10i32;
    // denom 16 -> exactly one 2:1 step; an odd width takes the symodd arm.
    for &w in &[65usize, 97, 33] {
        let w2 = (w * 8).div_ceil(16);
        let input = line(w, w as u64 * 7, bd);

        let mut c_out = vec![0u16; w2];
        cref::highbd_resize_plane_horizontal(&input, 1, w, w, &mut c_out, w2, w2, bd);

        let mut r_out = vec![0u16; w2];
        highbd_resize_plane_horizontal(&input, 1, w, w, &mut r_out, w2, w2, bd);
        assert_eq!(r_out, c_out, "odd-width ladder mismatch w {w}");

        // The step count is one and the projected length is (w+1)/2 == w2, so
        // the single symodd call IS the whole ladder. Prove the odd arm was
        // the one that ran by reproducing the output with symodd alone.
        assert_eq!(w2, w.div_ceil(2));
        let mut direct = vec![0u16; w2];
        highbd_down2_symodd(&input, w, &mut direct, bd);
        assert_eq!(
            direct, c_out,
            "the odd arm was NOT what the ladder ran for w {w} — this test proves nothing"
        );
    }
}

/// `highbd_resize_multistep` short-circuits `length == olength` to a copy.
#[test]
fn highbd_resize_multistep_identity_is_a_copy_and_matches_c() {
    let bd = 10i32;
    let w = 64usize;
    let input = line(w, 99, bd);
    let mut c_out = vec![0u16; w];
    cref::highbd_resize_plane_horizontal(&input, 1, w, w, &mut c_out, w, w, bd);
    assert_eq!(c_out, input, "C's identity path is a copy");
    let mut r_out = vec![0u16; w];
    highbd_resize_multistep(&input, w, &mut r_out, w, bd);
    assert_eq!(r_out, c_out);
}
