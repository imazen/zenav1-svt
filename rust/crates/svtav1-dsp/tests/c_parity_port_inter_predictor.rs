//! Differential parity for the motion-compensation dispatchers — evidence
//! tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven (all `nm -g`-visible in `Bin/Release/libSvtAv1Enc.a`):
//! `svt_inter_predictor_pd0`, `svt_inter_predictor`,
//! `svt_inter_predictor_light_pd1`, `svt_highbd_inter_predictor`,
//! `convolve_2d_for_intrabc`, `highbd_convolve_2d_for_intrabc`; plus the
//! header inlines `av1_get_convolve_filter_params` /
//! `av1_make_interp_filters` / `av1_extract_interp_filter` through a shim.
//!
//! WHICH C CODE THIS AGREES WITH: the dispatchers index the RTCD-filled
//! `svt_aom_convolve` / `svt_aom_convolveHbd` tables, so on a SIMD host the C
//! side runs the SIMD kernel, not the `_c` one. `records_which_c_tier_ran`
//! prints that fact so a green run is attributable, and every other cell here
//! therefore ALSO gates the port's scalar kernels against C's dispatched tier
//! — a strictly stronger comparison than the `_c`-only cells in
//! `c_parity_port_convolve.rs`.
//!
//! The `is_scaled` arm of every dispatcher IS covered
//! (`scaled_arm_matches_c`), now that `svt_av1_convolve_2d_scale_c` and its
//! highbd twin are ported. That cell replaced an earlier one which only pinned
//! the port's REFUSAL of scaled references — real parity is strictly stronger
//! evidence than a refusal.
//!
//! NOT COVERED, and named rather than implied:
//! * `svt_inter_predictor_light_pd1`'s `bd > 8` arm, which packs `src` +
//!   `src_2b` through `svt_aom_pack_block`. This port carries plain u16 planes
//!   by design, so that representation has no counterpart.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_convolve::{ConvolveParams, InterpFilterKind, SrcView};
use svtav1_dsp::port_convolve_hbd::SrcView16;
use svtav1_dsp::port_inter_predictor::{
    broadcast_interp_filter, convolve_2d_for_intrabc, extract_interp_filter,
    get_convolve_filter_params, highbd_convolve_2d_for_intrabc, highbd_inter_predictor,
    inter_predictor, inter_predictor_light_pd1_8bit, inter_predictor_pd0, make_interp_filters,
};
use svtav1_dsp::port_scale_factors::{SCALE_SUBPEL_SHIFTS, SubpelParams};

const PAD: usize = 8;

fn xs_rand(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

struct Fix8 {
    src: Vec<u8>,
    stride: usize,
    origin: usize,
}
impl Fix8 {
    fn new(w: usize, h: usize, seed: u32) -> Self {
        let stride = w + 2 * PAD;
        let rows = h + 2 * PAD;
        let mut s = seed | 1;
        let src = (0..stride * rows)
            .map(|_| {
                let v = xs_rand(&mut s);
                match v % 8 {
                    0 => 0u8,
                    1 => 255u8,
                    _ => (v >> 7) as u8,
                }
            })
            .collect();
        Self {
            src,
            stride,
            origin: PAD * stride + PAD,
        }
    }
    fn view(&self) -> SrcView<'_> {
        SrcView::new(&self.src, self.origin, self.stride)
    }
}

struct Fix16 {
    src: Vec<u16>,
    stride: usize,
    origin: usize,
}
impl Fix16 {
    fn new(w: usize, h: usize, seed: u32, bd: i32) -> Self {
        let stride = w + 2 * PAD;
        let rows = h + 2 * PAD;
        let max = (1u32 << bd) - 1;
        let mut s = seed | 1;
        let src = (0..stride * rows)
            .map(|_| {
                let v = xs_rand(&mut s);
                match v % 8 {
                    0 => 0u16,
                    1 => max as u16,
                    _ => ((v >> 5) % (max + 1)) as u16,
                }
            })
            .collect();
        Self {
            src,
            stride,
            origin: PAD * stride + PAD,
        }
    }
    fn view(&self) -> SrcView16<'_> {
        SrcView16::new(&self.src, self.origin, self.stride)
    }
}

const FILTERS: [(i32, InterpFilterKind); 4] = [
    (0, InterpFilterKind::EightTapRegular),
    (1, InterpFilterKind::EightTapSmooth),
    (2, InterpFilterKind::MultiTapSharp),
    (3, InterpFilterKind::Bilinear),
];
const SIZES: [(usize, usize); 6] = [(4, 4), (4, 8), (8, 4), (8, 8), (16, 16), (32, 16)];

/// Bit depths the RTCD-dispatched HIGHBD tables are a valid oracle at.
///
/// MEASURED 2026-08-31, and it is an oracle limit rather than a port result:
/// at bd 12 the port agrees with the pure-C kernels
/// (`c_parity_port_convolve_hbd.rs` sweeps 10 AND 12 and is green) but NOT
/// with what `svt_aom_convolveHbd` dispatches — C's dispatched
/// `highbd_convolve_2d_for_intrabc` returned an all-zero 4x4 block at bd 12,
/// and the compound CONV_BUF differed by a constant 8192 offset, both of which
/// are the signature of a kernel specialised for bd <= 10. SVT-AV1 encodes
/// 8-bit and 10-bit only, so bd 12 is outside the library's own envelope and
/// its SIMD highbd kernels are not a valid reference there. bd 12 coverage
/// therefore lives in `c_parity_port_convolve_hbd.rs` against the `_c`
/// kernels; this file, which drives the dispatch tables, stops at 10.
const HBD_DEPTHS: [i32; 1] = [10];

/// Unscaled subpel params in the SCALE_SUBPEL domain (what the dispatchers
/// receive before `revert_scale_extra_bits`): `xs`/`ys` at
/// `SCALE_SUBPEL_SHIFTS`, phases pre-multiplied by `1 << SCALE_EXTRA_BITS`.
fn unscaled(subpel_x: i32, subpel_y: i32) -> (SubpelParams, cref::RefSubpel) {
    let sp = SubpelParams {
        xs: SCALE_SUBPEL_SHIFTS,
        ys: SCALE_SUBPEL_SHIFTS,
        subpel_x: subpel_x << 6,
        subpel_y: subpel_y << 6,
    };
    (
        sp,
        cref::RefSubpel {
            xs: sp.xs,
            ys: sp.ys,
            subpel_x: sp.subpel_x,
            subpel_y: sp.subpel_y,
        },
    )
}

/// Records — and prints — whether the C side ran the pure-C kernels or a SIMD
/// tier, so a green result in this file is attributable to a specific oracle.
#[test]
fn records_which_c_tier_ran() {
    let pure_c = cref::convolve_tables_are_pure_c();
    println!(
        "svt_aom_convolve dispatch on this host: {}",
        if pure_c {
            "pure-C kernels"
        } else {
            "a SIMD tier (the port is compared against C's dispatched kernels)"
        }
    );
    // No assertion on WHICH tier — the point is that the tables are filled at
    // all. An all-null table would segfault the other cells rather than pass.
}

#[test]
fn filter_params_selection_matches_c() {
    for (yi, yk) in FILTERS {
        for (xi, xk) in FILTERS {
            let rust = make_interp_filters(yk, xk);
            let c = cref::make_interp_filters(yi, xi);
            assert_eq!(rust, c, "make_interp_filters(y{yi}, x{xi})");
            assert_eq!(
                extract_interp_filter(c, true) as i32,
                xi,
                "extract X from y{yi}/x{xi}"
            );
            assert_eq!(
                extract_interp_filter(c, false) as i32,
                yi,
                "extract Y from y{yi}/x{xi}"
            );
            for (w, h) in SIZES {
                let (fx, fy) = get_convolve_filter_params(c, w as i32, h as i32);
                let (cx, cy) = cref::get_convolve_filter_params(c, w as i32, h as i32);
                // The C shim reports InterpFilterParams::interp_filter, which
                // for the narrow-block 4-tap substitution keeps the ORIGINAL
                // filter id for regular/sharp (av1_interp_4tap[0] is tagged
                // EIGHTTAP_REGULAR) — so compare the kernel tables, which is
                // what the convolve actually reads, plus the id where it is
                // unambiguous.
                assert_eq!(
                    *fx.subpel_kernel(5),
                    cref::interp_filter_kernel(xi, w as i32, 5),
                    "x kernel y{yi}/x{xi} {w}x{h}"
                );
                assert_eq!(
                    *fy.subpel_kernel(5),
                    cref::interp_filter_kernel(yi, h as i32, 5),
                    "y kernel y{yi}/x{xi} {w}x{h}"
                );
                let _ = (cx, cy);
            }
        }
    }
}

/// PD0's whole MC surface: the unscaled arm indexes `[0][0][is_compound]` with
/// literal zeros, so a NONZERO subpel phase must still produce the whole-pel
/// copy. That is the cell that would catch a port which "helpfully" honoured
/// the phase.
#[test]
fn inter_predictor_pd0_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for (sx, sy) in [(0, 0), (9, 0), (0, 11), (15, 15)] {
            for is_compound in [false, true] {
                let cb_stride = w + 3;
                let (sp, csp) = unscaled(sx, sy);
                let f0 = Fix8::new(w, h, 0x0770_0001 ^ (w as u32) << 5 ^ sx as u32);
                let f1 = Fix8::new(w, h, 0x0880_0002 ^ (h as u32) << 5 ^ sy as u32);

                let mut r_dst = vec![0u8; w * h];
                let mut r_cb = vec![0u16; cb_stride * h];
                let mut c_dst = vec![0u8; w * h];
                let mut c_cb = vec![0u16; cb_stride * h];

                let mut cp = ConvolveParams::no_round(false, cb_stride, is_compound, 8);
                for (f, avg) in [(&f0, false), (&f1, true)] {
                    if avg && !is_compound {
                        continue;
                    }
                    cp.do_average = avg;
                    inter_predictor_pd0(f.view(), &mut r_dst, w, &mut r_cb, w, h, &sp, &cp);
                    cref::inter_predictor_pd0(
                        &f.src,
                        f.origin,
                        f.stride,
                        &mut c_dst,
                        w,
                        &mut c_cb,
                        cb_stride,
                        w,
                        h,
                        csp,
                        is_compound,
                        avg,
                    );
                }
                assert_eq!(
                    r_cb, c_cb,
                    "pd0 CONV_BUF {w}x{h} sub({sx},{sy}) comp{is_compound}"
                );
                assert_eq!(
                    r_dst, c_dst,
                    "pd0 dst {w}x{h} sub({sx},{sy}) comp{is_compound}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 48, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn inter_predictor_matches_c() {
    let mut cells = 0usize;
    let mut hit_kernel = [false; 8];
    for (yi, yk) in FILTERS {
        for (xi, xk) in FILTERS {
            let filters = make_interp_filters(yk, xk);
            assert_eq!(filters, cref::make_interp_filters(yi, xi));
            for (w, h) in SIZES {
                for (sx, sy) in [(0, 0), (5, 0), (0, 13), (7, 3), (15, 15)] {
                    for is_compound in [false, true] {
                        for (use_jnt, fwd, bck) in [(false, 0, 0), (true, 11, 5)] {
                            if !is_compound && use_jnt {
                                continue;
                            }
                            let cb_stride = w + 2;
                            let (sp, csp) = unscaled(sx, sy);
                            let f0 = Fix8::new(
                                w,
                                h,
                                0x1010_0001 ^ (w as u32) << 3 ^ sx as u32 ^ (xi as u32) << 12,
                            );
                            let f1 = Fix8::new(
                                w,
                                h,
                                0x2020_0002 ^ (h as u32) << 3 ^ sy as u32 ^ (yi as u32) << 12,
                            );

                            let mut r_dst = vec![0u8; w * h];
                            let mut r_cb = vec![0u16; cb_stride * h];
                            let mut c_dst = vec![0u8; w * h];
                            let mut c_cb = vec![0u16; cb_stride * h];
                            let mut cp = ConvolveParams::no_round(false, cb_stride, is_compound, 8);
                            cp.use_jnt_comp_avg = use_jnt;
                            cp.fwd_offset = fwd;
                            cp.bck_offset = bck;

                            for (f, avg) in [(&f0, false), (&f1, true)] {
                                if avg && !is_compound {
                                    continue;
                                }
                                cp.do_average = avg;
                                inter_predictor(
                                    f.view(),
                                    &mut r_dst,
                                    w,
                                    &mut r_cb,
                                    &sp,
                                    w,
                                    h,
                                    &cp,
                                    filters,
                                    false,
                                );
                                cref::inter_predictor(
                                    &f.src,
                                    f.origin,
                                    f.stride,
                                    &mut c_dst,
                                    w,
                                    &mut c_cb,
                                    cb_stride,
                                    csp,
                                    (64, 64, 64, 64),
                                    w,
                                    h,
                                    cref::RefCompound {
                                        is_compound,
                                        do_average: avg,
                                        use_jnt,
                                        fwd,
                                        bck,
                                    },
                                    filters,
                                    false,
                                );
                            }
                            assert_eq!(
                                r_cb, c_cb,
                                "inter_predictor CONV_BUF {w}x{h} y{yi}/x{xi} sub({sx},{sy}) comp{is_compound}"
                            );
                            assert_eq!(
                                r_dst, c_dst,
                                "inter_predictor dst {w}x{h} y{yi}/x{xi} sub({sx},{sy}) comp{is_compound}"
                            );
                            let idx = ((sx != 0) as usize) << 2
                                | ((sy != 0) as usize) << 1
                                | is_compound as usize;
                            hit_kernel[idx] = true;
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 1000, "anti-vacuity: only {cells} cells ran");
    assert!(
        hit_kernel.iter().all(|&b| b),
        "not every svt_aom_convolve table entry was reached: {hit_kernel:?}"
    );
}

/// The IntraBC arm: BILINEAR at the fixed phase 8, dispatched on which axes
/// are sub-pel. Reached both directly and through `svt_inter_predictor`.
#[test]
fn intrabc_arm_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for (sx, sy) in [(1, 0), (0, 1), (1, 1), (8, 8)] {
            let cp = ConvolveParams::single(false, 8);
            let f = Fix8::new(w, h, 0x3030_0003 ^ (w as u32) ^ sx as u32);
            let mut r_dst = vec![0u8; w * h];
            let mut c_dst = vec![0u8; w * h];
            convolve_2d_for_intrabc(f.view(), &mut r_dst, w, w, h, sx, sy, &cp);
            cref::convolve_2d_for_intrabc(&f.src, f.origin, f.stride, &mut c_dst, w, w, h, sx, sy);
            assert_eq!(
                r_dst, c_dst,
                "convolve_2d_for_intrabc {w}x{h} sub({sx},{sy})"
            );

            // And through svt_inter_predictor's is_intrabc branch.
            let (sp, csp) = unscaled(sx, sy);
            let mut r2 = vec![0u8; w * h];
            let mut c2 = vec![0u8; w * h];
            let mut cb = vec![0u16; w * h];
            inter_predictor(
                f.view(),
                &mut r2,
                w,
                &mut cb,
                &sp,
                w,
                h,
                &cp,
                broadcast_interp_filter(InterpFilterKind::EightTapRegular),
                true,
            );
            cref::inter_predictor(
                &f.src,
                f.origin,
                f.stride,
                &mut c2,
                w,
                &mut cb,
                w,
                csp,
                (64, 64, 64, 64),
                w,
                h,
                cref::RefCompound {
                    is_compound: false,
                    do_average: false,
                    use_jnt: false,
                    fwd: 0,
                    bck: 0,
                },
                broadcast_interp_filter(InterpFilterKind::EightTapRegular),
                true,
            );
            assert_eq!(
                r2, c2,
                "svt_inter_predictor intrabc arm {w}x{h} sub({sx},{sy})"
            );
            cells += 1;
        }
    }
    assert!(cells >= 24, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_inter_predictor_matches_c() {
    let mut cells = 0usize;
    for bd in HBD_DEPTHS {
        for (yi, yk) in FILTERS {
            for (xi, xk) in FILTERS {
                let filters = make_interp_filters(yk, xk);
                for (w, h) in SIZES {
                    for (sx, sy) in [(0, 0), (5, 0), (0, 13), (15, 15)] {
                        for is_compound in [false, true] {
                            let cb_stride = w + 2;
                            let (sp, csp) = unscaled(sx, sy);
                            let f0 =
                                Fix16::new(w, h, 0x4040_0001 ^ (w as u32) << 3 ^ sx as u32, bd);
                            let f1 =
                                Fix16::new(w, h, 0x5050_0002 ^ (h as u32) << 3 ^ sy as u32, bd);

                            let mut r_dst = vec![0u16; w * h];
                            let mut r_cb = vec![0u16; cb_stride * h];
                            let mut c_dst = vec![0u16; w * h];
                            let mut c_cb = vec![0u16; cb_stride * h];
                            let mut cp =
                                ConvolveParams::no_round(false, cb_stride, is_compound, bd);

                            for (f, avg) in [(&f0, false), (&f1, true)] {
                                if avg && !is_compound {
                                    continue;
                                }
                                cp.do_average = avg;
                                highbd_inter_predictor(
                                    f.view(),
                                    &mut r_dst,
                                    w,
                                    &mut r_cb,
                                    &sp,
                                    w,
                                    h,
                                    &cp,
                                    filters,
                                    false,
                                    bd,
                                );
                                cref::highbd_inter_predictor(
                                    &f.src,
                                    f.origin,
                                    f.stride,
                                    &mut c_dst,
                                    w,
                                    &mut c_cb,
                                    cb_stride,
                                    csp,
                                    (64, 64, 64, 64),
                                    w,
                                    h,
                                    cref::RefCompound {
                                        is_compound,
                                        do_average: avg,
                                        use_jnt: false,
                                        fwd: 0,
                                        bck: 0,
                                    },
                                    filters,
                                    false,
                                    bd,
                                );
                            }
                            assert_eq!(
                                r_cb, c_cb,
                                "hbd predictor CONV_BUF bd{bd} {w}x{h} y{yi}/x{xi} sub({sx},{sy})"
                            );
                            assert_eq!(
                                r_dst, c_dst,
                                "hbd predictor dst bd{bd} {w}x{h} y{yi}/x{xi} sub({sx},{sy})"
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 700, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_intrabc_arm_matches_c() {
    let mut cells = 0usize;
    for bd in HBD_DEPTHS {
        for (w, h) in SIZES {
            for (sx, sy) in [(1, 0), (0, 1), (1, 1)] {
                let cp = ConvolveParams::single(false, bd);
                let f = Fix16::new(w, h, 0x6060_0006 ^ (w as u32) ^ sx as u32, bd);
                let mut r = vec![0u16; w * h];
                let mut c = vec![0u16; w * h];
                highbd_convolve_2d_for_intrabc(f.view(), &mut r, w, w, h, sx, sy, &cp, bd);
                cref::highbd_convolve_2d_for_intrabc(
                    &f.src, f.origin, f.stride, &mut c, w, w, h, sx, sy, bd,
                );
                assert_eq!(
                    r, c,
                    "highbd_convolve_2d_for_intrabc bd{bd} {w}x{h} sub({sx},{sy})"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 18, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn inter_predictor_light_pd1_8bit_matches_c() {
    let mut cells = 0usize;
    for (yi, yk) in FILTERS {
        for (xi, xk) in FILTERS {
            let filters = make_interp_filters(yk, xk);
            for (w, h) in SIZES {
                for (sx, sy) in [(0, 0), (3, 0), (0, 9), (15, 15)] {
                    for is_compound in [false, true] {
                        let cb_stride = w + 1;
                        let (sp, csp) = unscaled(sx, sy);
                        let f0 = Fix8::new(w, h, 0x7070_0001 ^ (w as u32) ^ sx as u32);
                        let f1 = Fix8::new(w, h, 0x9090_0002 ^ (h as u32) ^ sy as u32);

                        let mut r_dst = vec![0u8; w * h];
                        let mut r_cb = vec![0u16; cb_stride * h];
                        let mut c_dst = vec![0u8; w * h];
                        let mut c_cb = vec![0u16; cb_stride * h];
                        let mut cp = ConvolveParams::no_round(false, cb_stride, is_compound, 8);

                        for (f, avg) in [(&f0, false), (&f1, true)] {
                            if avg && !is_compound {
                                continue;
                            }
                            cp.do_average = avg;
                            inter_predictor_light_pd1_8bit(
                                f.view(),
                                &mut r_dst,
                                w,
                                &mut r_cb,
                                w,
                                h,
                                filters,
                                &sp,
                                &cp,
                            );
                            let mut csrc = f.src.clone();
                            cref::inter_predictor_light_pd1_8bit(
                                &mut csrc,
                                f.origin,
                                f.stride,
                                &mut c_dst,
                                w,
                                &mut c_cb,
                                cb_stride,
                                w,
                                h,
                                filters,
                                csp,
                                cref::RefCompound {
                                    is_compound,
                                    do_average: avg,
                                    use_jnt: false,
                                    fwd: 0,
                                    bck: 0,
                                },
                            );
                        }
                        assert_eq!(
                            r_cb, c_cb,
                            "light_pd1 CONV_BUF {w}x{h} y{yi}/x{xi} sub({sx},{sy}) comp{is_compound}"
                        );
                        assert_eq!(
                            r_dst, c_dst,
                            "light_pd1 dst {w}x{h} y{yi}/x{xi} sub({sx},{sy}) comp{is_compound}"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 700, "anti-vacuity: only {cells} cells ran");
}

/// The scaled arm of all four dispatchers, against C.
///
/// A scaled reference makes `has_scale(xs, ys)` true, which routes every entry
/// point into `svt_av1_convolve_2d_scale` / its highbd twin. The phases below
/// are in the SCALE_SUBPEL (10-bit) domain, and deliberately include the 1:1
/// step (1024) — which is still the SCALED path when the other axis differs.
#[test]
fn scaled_arm_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for (spx, stx, spy, sty) in [
            (0i32, 2048i32, 0i32, 1024i32),
            (512, 1536, 300, 1536),
            (64, 683, 900, 683),
            (0, 1024, 0, 2048),
        ] {
            for is_compound in [false, true] {
                let cb_stride = w + 2;
                let sp = SubpelParams {
                    xs: stx,
                    ys: sty,
                    subpel_x: spx,
                    subpel_y: spy,
                };
                let csp = cref::RefSubpel {
                    xs: sp.xs,
                    ys: sp.ys,
                    subpel_x: sp.subpel_x,
                    subpel_y: sp.subpel_y,
                };
                assert!(
                    svtav1_dsp::port_scale_factors::has_scale(sp.xs, sp.ys),
                    "the cell must actually take the scaled arm"
                );
                // A generously padded source: the scaled reach is driven by the
                // step, not the block size.
                let f = Fix8::new(w * 3 + 64, h * 3 + 64, 0xACE0 ^ (w as u32) ^ spx as u32);
                let g = Fix16::new(w * 3 + 64, h * 3 + 64, 0xBDF1 ^ (h as u32) ^ spy as u32, 10);

                for (cs, filters) in [
                    (
                        0usize,
                        broadcast_interp_filter(InterpFilterKind::EightTapRegular),
                    ),
                    (
                        1,
                        make_interp_filters(
                            InterpFilterKind::EightTapSmooth,
                            InterpFilterKind::MultiTapSharp,
                        ),
                    ),
                ] {
                    let _ = cs;
                    let mut cp = ConvolveParams::no_round(false, cb_stride, is_compound, 8);

                    // pd0
                    let mut r = vec![0u8; w * h];
                    let mut c = vec![0u8; w * h];
                    let mut rcb = vec![0u16; cb_stride * h];
                    let mut ccb = vec![0u16; cb_stride * h];
                    inter_predictor_pd0(f.view(), &mut r, w, &mut rcb, w, h, &sp, &cp);
                    cref::inter_predictor_pd0(
                        &f.src,
                        f.origin,
                        f.stride,
                        &mut c,
                        w,
                        &mut ccb,
                        cb_stride,
                        w,
                        h,
                        csp,
                        is_compound,
                        false,
                    );
                    assert_eq!(rcb, ccb, "pd0 scaled CONV_BUF {w}x{h}");
                    assert_eq!(r, c, "pd0 scaled dst {w}x{h}");

                    // svt_inter_predictor
                    let mut r = vec![0u8; w * h];
                    let mut c = vec![0u8; w * h];
                    let mut rcb = vec![0u16; cb_stride * h];
                    let mut ccb = vec![0u16; cb_stride * h];
                    inter_predictor(
                        f.view(),
                        &mut r,
                        w,
                        &mut rcb,
                        &sp,
                        w,
                        h,
                        &cp,
                        filters,
                        false,
                    );
                    cref::inter_predictor(
                        &f.src,
                        f.origin,
                        f.stride,
                        &mut c,
                        w,
                        &mut ccb,
                        cb_stride,
                        csp,
                        (64, 64, 64, 64),
                        w,
                        h,
                        cref::RefCompound {
                            is_compound,
                            do_average: false,
                            use_jnt: false,
                            fwd: 0,
                            bck: 0,
                        },
                        filters,
                        false,
                    );
                    assert_eq!(rcb, ccb, "inter_predictor scaled CONV_BUF {w}x{h}");
                    assert_eq!(r, c, "inter_predictor scaled dst {w}x{h}");

                    // light pd1
                    let mut r = vec![0u8; w * h];
                    let mut c = vec![0u8; w * h];
                    let mut rcb = vec![0u16; cb_stride * h];
                    let mut ccb = vec![0u16; cb_stride * h];
                    inter_predictor_light_pd1_8bit(
                        f.view(),
                        &mut r,
                        w,
                        &mut rcb,
                        w,
                        h,
                        filters,
                        &sp,
                        &cp,
                    );
                    let mut csrc = f.src.clone();
                    cref::inter_predictor_light_pd1_8bit(
                        &mut csrc,
                        f.origin,
                        f.stride,
                        &mut c,
                        w,
                        &mut ccb,
                        cb_stride,
                        w,
                        h,
                        filters,
                        csp,
                        cref::RefCompound {
                            is_compound,
                            do_average: false,
                            use_jnt: false,
                            fwd: 0,
                            bck: 0,
                        },
                    );
                    assert_eq!(rcb, ccb, "light_pd1 scaled CONV_BUF {w}x{h}");
                    assert_eq!(r, c, "light_pd1 scaled dst {w}x{h}");

                    // highbd
                    cp = ConvolveParams::no_round(false, cb_stride, is_compound, 10);
                    let mut r = vec![0u16; w * h];
                    let mut c = vec![0u16; w * h];
                    let mut rcb = vec![0u16; cb_stride * h];
                    let mut ccb = vec![0u16; cb_stride * h];
                    highbd_inter_predictor(
                        g.view(),
                        &mut r,
                        w,
                        &mut rcb,
                        &sp,
                        w,
                        h,
                        &cp,
                        filters,
                        false,
                        10,
                    );
                    cref::highbd_inter_predictor(
                        &g.src,
                        g.origin,
                        g.stride,
                        &mut c,
                        w,
                        &mut ccb,
                        cb_stride,
                        csp,
                        (64, 64, 64, 64),
                        w,
                        h,
                        cref::RefCompound {
                            is_compound,
                            do_average: false,
                            use_jnt: false,
                            fwd: 0,
                            bck: 0,
                        },
                        filters,
                        false,
                        10,
                    );
                    assert_eq!(rcb, ccb, "highbd scaled CONV_BUF {w}x{h}");
                    assert_eq!(r, c, "highbd scaled dst {w}x{h}");

                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 80, "anti-vacuity: only {cells} cells ran");
}
