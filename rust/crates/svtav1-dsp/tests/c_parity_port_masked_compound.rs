//! Differential parity for the masked-compound / wedge-search primitives —
//! evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven (all `nm -g`-visible): `svt_aom_is_masked_compound_type`,
//! `svt_aom_subtract_block_c`, `svt_aom_highbd_subtract_block_c`,
//! `svt_aom_sum_squares_i16_c`, `svt_aom_sse_c`, `svt_aom_highbd_sse_c`,
//! `svt_av1_wedge_sse_from_residuals_c`,
//! `svt_av1_wedge_sign_from_residuals_c`,
//! `svt_av1_wedge_compute_delta_squares_c`,
//! `svt_av1_build_compound_diffwtd_mask_c`,
//! `svt_av1_build_compound_diffwtd_mask_highbd_c`,
//! `svt_aom_highbd_blend_a64_hmask_16bit_c`.
//!
//! The `static` `diffwtd_mask` / `diffwtd_mask_highbd` are gated INDIRECTLY:
//! the two exported `build_compound_diffwtd_mask*` are their only callers and
//! pass their whole output through, so a difference in either inner loop
//! shows up here. `diffwtd_mask_highbd`'s four hand-specialised arms
//! (bd == 8 / bd > 8, crossed with `which_inverse`) are all driven.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_masked_compound::{
    CompoundType, DiffwtdMaskType, build_compound_diffwtd_mask, build_compound_diffwtd_mask_highbd,
    highbd_blend_a64_hmask_16bit, highbd_sse, highbd_subtract_block, is_masked_compound_type, sse,
    subtract_block, sum_squares_i16, wedge_compute_delta_squares, wedge_sign_from_residuals,
    wedge_sse_from_residuals,
};

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

fn u8s(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => 255,
                _ => (v >> 9) as u8,
            }
        })
        .collect()
}

fn u16s(n: usize, seed: u32, bd: u32) -> Vec<u16> {
    let max = (1u32 << bd) - 1;
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => max as u16,
                _ => ((v >> 5) % (max + 1)) as u16,
            }
        })
        .collect()
}

/// Residuals deliberately spanning the full `int16` range, so the in-loop
/// clamps in the wedge helpers are exercised rather than assumed inert.
fn i16s(n: usize, seed: u32) -> Vec<i16> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 6 {
                0 => i16::MIN,
                1 => i16::MAX,
                2 => 0,
                _ => (v >> 8) as i16,
            }
        })
        .collect()
}

const SIZES: [(usize, usize); 7] = [(4, 4), (4, 8), (8, 4), (8, 8), (16, 8), (32, 32), (64, 16)];

#[test]
fn is_masked_compound_type_matches_c() {
    for (t, kind) in [
        (0, CompoundType::Average),
        (1, CompoundType::DistWtd),
        (2, CompoundType::DiffWtd),
        (3, CompoundType::Wedge),
    ] {
        assert_eq!(
            is_masked_compound_type(kind),
            cref::is_masked_compound_type(t),
            "compound type {t}"
        );
    }
}

#[test]
fn subtract_block_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for stride_bump in [0usize, 3] {
            let ss = w + stride_bump;
            let ds = w + stride_bump;
            let src = u8s(ss * h, 0x1111 ^ w as u32);
            let pred = u8s(ss * h, 0x2222 ^ h as u32);
            let mut r = vec![0i16; ds * h];
            let mut c = vec![0i16; ds * h];
            subtract_block(h, w, &mut r, ds, &src, ss, &pred, ss);
            cref::subtract_block(h, w, &mut c, ds, &src, ss, &pred, ss);
            assert_eq!(r, c, "subtract_block {w}x{h} stride+{stride_bump}");
            cells += 1;
        }
    }
    assert!(cells >= 14, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_subtract_block_matches_c() {
    let mut cells = 0usize;
    for bd in [10u32, 12] {
        for (w, h) in SIZES {
            let ss = w + 2;
            let src = u16s(ss * h, 0x3333 ^ w as u32, bd);
            let pred = u16s(ss * h, 0x4444 ^ h as u32, bd);
            let mut r = vec![0i16; ss * h];
            let mut c = vec![0i16; ss * h];
            highbd_subtract_block(h, w, &mut r, ss, &src, ss, &pred, ss);
            cref::highbd_subtract_block(h, w, &mut c, ss, &src, ss, &pred, ss, bd as i32);
            assert_eq!(r, c, "highbd_subtract_block bd{bd} {w}x{h}");
            cells += 1;
        }
    }
    assert!(cells >= 14, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn sum_squares_i16_matches_c() {
    let mut cells = 0usize;
    for n in [1usize, 2, 15, 16, 64, 255, 1024] {
        for seed in [0x5555u32, 0x6666, 0x7777] {
            let v = i16s(n, seed ^ n as u32);
            assert_eq!(
                sum_squares_i16(&v, n),
                cref::sum_squares_i16(&v, n),
                "sum_squares_i16 n={n} seed={seed:x}"
            );
            cells += 1;
        }
    }
    assert!(cells >= 21, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn sse_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for bump in [0usize, 5] {
            let a_stride = w + bump;
            let b_stride = w + 2 * bump;
            let a = u8s(a_stride * h, 0x8888 ^ w as u32);
            let b = u8s(b_stride * h, 0x9999 ^ h as u32);
            assert_eq!(
                sse(&a, a_stride, &b, b_stride, w, h),
                cref::sse(&a, a_stride, &b, b_stride, w, h),
                "sse {w}x{h} bump {bump}"
            );
            cells += 1;
        }
    }
    assert!(cells >= 14, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_sse_matches_c() {
    let mut cells = 0usize;
    for bd in [10u32, 12] {
        for (w, h) in SIZES {
            let a_stride = w + 3;
            let b_stride = w + 1;
            let a = u16s(a_stride * h, 0xAAAA ^ w as u32, bd);
            let b = u16s(b_stride * h, 0xBBBB ^ h as u32, bd);
            assert_eq!(
                highbd_sse(&a, a_stride, &b, b_stride, w, h),
                cref::highbd_sse(&a, a_stride, &b, b_stride, w, h),
                "highbd_sse bd{bd} {w}x{h}"
            );
            cells += 1;
        }
    }
    assert!(cells >= 14, "anti-vacuity: only {cells} cells ran");
}

/// The in-loop `clamp(t, INT16_MIN, INT16_MAX)` only bites when residuals
/// exceed `16 - WEDGE_WEIGHT_BITS = 10` signed bits, so the fixture spans the
/// whole `int16` range on purpose.
#[test]
fn wedge_sse_from_residuals_matches_c() {
    let mut cells = 0usize;
    for n in [1usize, 16, 64, 256, 1024] {
        for seed in [0xC0DEu32, 0xFEED, 0x1234] {
            let r1 = i16s(n, seed);
            let d = i16s(n, seed.wrapping_mul(3));
            let mut ms = seed | 1;
            let m: Vec<u8> = (0..n).map(|_| (xs(&mut ms) % 65) as u8).collect();
            assert_eq!(
                wedge_sse_from_residuals(&r1, &d, &m, n),
                cref::wedge_sse_from_residuals(&r1, &d, &m, n),
                "wedge_sse n={n} seed={seed:x}"
            );
            cells += 1;
        }
    }
    assert!(cells >= 15, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn wedge_sign_from_residuals_matches_c() {
    let mut cells = 0usize;
    let mut trues = 0usize;
    let mut falses = 0usize;
    for n in [1usize, 16, 64, 256] {
        for seed in [0x2468u32, 0x1357, 0xBEEF] {
            let ds = i16s(n, seed);
            let mut ms = seed | 3;
            let m: Vec<u8> = (0..n).map(|_| (xs(&mut ms) % 65) as u8).collect();
            for limit in [i64::MIN, -1_000_000, 0, 1_000_000, i64::MAX] {
                let r = wedge_sign_from_residuals(&ds, &m, n, limit);
                assert_eq!(
                    r,
                    cref::wedge_sign_from_residuals(&ds, &m, n, limit),
                    "wedge_sign n={n} seed={seed:x} limit={limit}"
                );
                if r {
                    trues += 1
                } else {
                    falses += 1
                }
                cells += 1;
            }
        }
    }
    assert!(cells >= 60, "anti-vacuity: only {cells} cells ran");
    assert!(
        trues > 5 && falses > 5,
        "both verdicts must occur: {trues}/{falses}"
    );
}

#[test]
fn wedge_compute_delta_squares_matches_c() {
    let mut cells = 0usize;
    let mut saturated = 0usize;
    for n in [1usize, 16, 64, 512] {
        for seed in [0x0F0Fu32, 0xF0F0, 0x55AA] {
            let a = i16s(n, seed);
            let b = i16s(n, seed ^ 0xFFFF);
            let mut r = vec![0i16; n];
            let mut c = vec![0i16; n];
            wedge_compute_delta_squares(&mut r, &a, &b, n);
            cref::wedge_compute_delta_squares(&mut c, &a, &b, n);
            assert_eq!(r, c, "delta_squares n={n} seed={seed:x}");
            saturated += r
                .iter()
                .filter(|&&v| v == i16::MIN || v == i16::MAX)
                .count();
            cells += 1;
        }
    }
    assert!(cells >= 12, "anti-vacuity: only {cells} cells ran");
    assert!(
        saturated > 10,
        "the int16 saturation must actually fire; it fired {saturated} times"
    );
}

#[test]
fn build_compound_diffwtd_mask_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for mt in [DiffwtdMaskType::D38, DiffwtdMaskType::D38Inv] {
            let s0s = w + 4;
            let s1s = w + 1;
            let src0 = u8s(s0s * h, 0xDEAD ^ w as u32);
            let src1 = u8s(s1s * h, 0xBEEF ^ h as u32);
            let mut r = vec![0u8; w * h];
            let mut c = vec![0u8; w * h];
            build_compound_diffwtd_mask(&mut r, mt, &src0, s0s, &src1, s1s, h, w);
            cref::build_compound_diffwtd_mask(&mut c, mt as i32, &src0, s0s, &src1, s1s, h, w);
            assert_eq!(r, c, "diffwtd mask {w}x{h} type {mt:?}");
            cells += 1;
        }
    }
    assert!(cells >= 14, "anti-vacuity: only {cells} cells ran");
}

/// Drives all four hand-specialised arms of `diffwtd_mask_highbd`: bd == 8
/// (whose `bd_shift` is 0) and bd > 8, crossed with the inverse flag.
#[test]
fn build_compound_diffwtd_mask_highbd_matches_c() {
    let mut cells = 0usize;
    for bd in [8u32, 10, 12] {
        for (w, h) in SIZES {
            for mt in [DiffwtdMaskType::D38, DiffwtdMaskType::D38Inv] {
                let s0s = w + 2;
                let s1s = w + 3;
                let src0 = u16s(s0s * h, 0xCAFE ^ w as u32 ^ bd, bd);
                let src1 = u16s(s1s * h, 0xF00D ^ h as u32 ^ bd, bd);
                let mut r = vec![0u8; w * h];
                let mut c = vec![0u8; w * h];
                build_compound_diffwtd_mask_highbd(&mut r, mt, &src0, s0s, &src1, s1s, h, w, bd);
                cref::build_compound_diffwtd_mask_highbd(
                    &mut c, mt as i32, &src0, s0s, &src1, s1s, h, w, bd as i32,
                );
                assert_eq!(r, c, "diffwtd hbd mask bd{bd} {w}x{h} type {mt:?}");
                cells += 1;
            }
        }
    }
    assert!(cells >= 42, "anti-vacuity: only {cells} cells ran");
}

/// `svt_aom_highbd_blend_a64_hmask_16bit_c` asserts power-of-two `w` and `h`,
/// so the sizes here stay powers of two.
#[test]
fn highbd_blend_a64_hmask_16bit_matches_c() {
    let mut cells = 0usize;
    for bd in [8i32, 10, 12] {
        for (w, h) in [(4usize, 4usize), (8, 8), (16, 8), (32, 32), (4, 16)] {
            let s = w + 2;
            let src0 = u16s(s * h, 0x1010 ^ w as u32, bd as u32);
            let src1 = u16s(s * h, 0x2020 ^ h as u32, bd as u32);
            let mut ms = 0x3030u32 ^ w as u32;
            let mask: Vec<u8> = (0..w).map(|_| (xs(&mut ms) % 65) as u8).collect();
            let mut r = vec![0u16; s * h];
            let mut c = vec![0u16; s * h];
            highbd_blend_a64_hmask_16bit(&mut r, s, &src0, s, &src1, s, &mask, w, h);
            cref::highbd_blend_a64_hmask_16bit(&mut c, s, &src0, s, &src1, s, &mask, w, h, bd);
            assert_eq!(r, c, "hbd blend hmask bd{bd} {w}x{h}");
            cells += 1;
        }
    }
    assert!(cells >= 15, "anti-vacuity: only {cells} cells ran");
}
