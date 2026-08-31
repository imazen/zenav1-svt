//! Differential parity for the scaled-reference MC kernels — evidence tier 1
//! (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven: `svt_av1_convolve_2d_scale_c` and
//! `svt_av1_highbd_convolve_2d_scale_c` (both `nm -g`-visible), over the full
//! `ConvolveParams` surface: single, compound-write, compound-average and
//! compound-distance-weighted, at 8/10/12 bits.
//!
//! `crates/svtav1-dsp/tests/c_parity_scale.rs` pins `scale.rs`'s homegrown
//! `scaled_prediction` as NOT matching this kernel, with an `assert_ne!` and a
//! note to flip it "when scale.rs is ported". That pin is left exactly as it
//! is: this file ports the kernel into `port_convolve_scale.rs` with the C
//! contract, and re-pointing `scale.rs`'s callers is a separate, caller-side
//! change. Flipping the pin before that would claim a parity the callers do
//! not have.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_convolve::{
    ConvolveParams, FilterParams, InterpFilterKind, SrcView, interp_filter_params_with_block_size,
};
use svtav1_dsp::port_convolve_hbd::SrcView16;
use svtav1_dsp::port_convolve_scale::{convolve_2d_scale, highbd_convolve_2d_scale, scale_im_h};

/// Generous margin: the horizontal reach is `(x_qn >> 10) + 4` at the last
/// column and the vertical one `im_h` rows, both driven by the step.
const PAD: usize = 96;

fn xs(s: &mut u32) -> u32 {
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
        let stride = w * 3 + 2 * PAD;
        let rows = h * 3 + 2 * PAD;
        let mut s = seed | 1;
        let src = (0..stride * rows)
            .map(|_| {
                let v = xs(&mut s);
                match v % 8 {
                    0 => 0,
                    1 => 255,
                    _ => (v >> 9) as u8,
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
    fn new(w: usize, h: usize, seed: u32, bd: u32) -> Self {
        let stride = w * 3 + 2 * PAD;
        let rows = h * 3 + 2 * PAD;
        let max = (1u32 << bd) - 1;
        let mut s = seed | 1;
        let src = (0..stride * rows)
            .map(|_| {
                let v = xs(&mut s);
                match v % 8 {
                    0 => 0,
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
const SIZES: [(usize, usize); 5] = [(4, 4), (8, 8), (16, 8), (16, 16), (32, 32)];

/// `(subpel, step)` pairs: 1:1 (step 1024), the 2x-down and 16x-up limits AV1
/// allows, and a couple of awkward non-power-of-two ratios.
const PHASES: [(i32, i32); 6] = [
    (0, 1024),
    (512, 1024),
    (0, 2048),
    (300, 1536),
    (900, 683),
    (64, 64),
];

fn params_for(kind: InterpFilterKind, size: usize) -> FilterParams {
    interp_filter_params_with_block_size(kind, size as i32)
}

const COMPOUNDS: [(bool, bool, bool, i32, i32); 4] = [
    (false, false, false, 0, 0),
    (true, false, false, 0, 0),
    (true, true, false, 0, 0),
    (true, true, true, 11, 5),
];

#[test]
fn convolve_2d_scale_matches_c() {
    let mut cells = 0usize;
    let mut compound_write = 0usize;
    let mut compound_avg = 0usize;
    for (fx_i, fx_k) in FILTERS {
        for (fy_i, fy_k) in FILTERS {
            for (w, h) in SIZES {
                for (spx, stx) in PHASES {
                    for (spy, sty) in PHASES {
                        for (is_compound, do_average, use_jnt, fwd, bck) in COMPOUNDS {
                            let cb_stride = w + 3;
                            let f = Fix8::new(
                                w,
                                h,
                                0x51CA1E ^ (w as u32) << 5 ^ spx as u32 ^ (fx_i as u32) << 13,
                            );
                            let mut r_dst = vec![0u8; w * h];
                            let mut c_dst = vec![0u8; w * h];
                            // Seed the CONV_BUF identically so the average arm
                            // reads real data, not zeros.
                            let mut seed = 0x2468u32;
                            let cb0: Vec<u16> = (0..cb_stride * h)
                                .map(|_| (xs(&mut seed) % 40000) as u16)
                                .collect();
                            let mut r_cb = cb0.clone();
                            let mut c_cb = cb0.clone();

                            let mut cp =
                                ConvolveParams::no_round(do_average, cb_stride, is_compound, 8);
                            cp.use_jnt_comp_avg = use_jnt;
                            cp.fwd_offset = fwd;
                            cp.bck_offset = bck;

                            convolve_2d_scale(
                                f.view(),
                                &mut r_dst,
                                w,
                                &mut r_cb,
                                w,
                                h,
                                &params_for(fx_k, w),
                                &params_for(fy_k, h),
                                spx,
                                stx,
                                spy,
                                sty,
                                &cp,
                            );
                            cref::convolve_2d_scale_full(
                                &f.src,
                                f.origin,
                                f.stride,
                                &mut c_dst,
                                w,
                                &mut c_cb,
                                cb_stride,
                                w,
                                h,
                                fx_i,
                                w as i32,
                                fy_i,
                                h as i32,
                                cref::ScalePhases {
                                    subpel_x_qn: spx,
                                    x_step_qn: stx,
                                    subpel_y_qn: spy,
                                    y_step_qn: sty,
                                },
                                cref::RefCompound {
                                    is_compound,
                                    do_average,
                                    use_jnt,
                                    fwd,
                                    bck,
                                },
                            );
                            assert_eq!(
                                r_cb, c_cb,
                                "scale CONV_BUF {w}x{h} fx{fx_i} fy{fy_i} x({spx},{stx}) y({spy},{sty}) comp{is_compound}/{do_average}"
                            );
                            assert_eq!(
                                r_dst, c_dst,
                                "scale dst {w}x{h} fx{fx_i} fy{fy_i} x({spx},{stx}) y({spy},{sty}) comp{is_compound}/{do_average}"
                            );
                            if is_compound && !do_average {
                                compound_write += 1;
                            }
                            if do_average {
                                compound_avg += 1;
                            }
                            cells += 1;
                            // The intermediate height really does move.
                            assert!(scale_im_h(h, spy, sty) >= 8);
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 2800, "anti-vacuity: only {cells} cells ran");
    assert!(
        compound_write > 500 && compound_avg > 500,
        "both compound arms must run: {compound_write}/{compound_avg}"
    );
}

#[test]
fn highbd_convolve_2d_scale_matches_c() {
    let mut cells = 0usize;
    for bd in [10u32, 12] {
        for (fx_i, fx_k) in FILTERS {
            for (w, h) in SIZES {
                for (spx, stx) in PHASES {
                    for (spy, sty) in PHASES {
                        for (is_compound, do_average, use_jnt, fwd, bck) in COMPOUNDS {
                            let cb_stride = w + 2;
                            let f = Fix16::new(w, h, 0xB16BD ^ (w as u32) ^ spx as u32 ^ bd, bd);
                            let mut r_dst = vec![0u16; w * h];
                            let mut c_dst = vec![0u16; w * h];
                            let mut seed = 0x1357u32 ^ bd;
                            let cb0: Vec<u16> = (0..cb_stride * h)
                                .map(|_| (xs(&mut seed) % 40000) as u16)
                                .collect();
                            let mut r_cb = cb0.clone();
                            let mut c_cb = cb0.clone();

                            let mut cp = ConvolveParams::no_round(
                                do_average,
                                cb_stride,
                                is_compound,
                                bd as i32,
                            );
                            cp.use_jnt_comp_avg = use_jnt;
                            cp.fwd_offset = fwd;
                            cp.bck_offset = bck;

                            highbd_convolve_2d_scale(
                                f.view(),
                                &mut r_dst,
                                w,
                                &mut r_cb,
                                w,
                                h,
                                &params_for(fx_k, w),
                                &params_for(fx_k, h),
                                spx,
                                stx,
                                spy,
                                sty,
                                &cp,
                                bd as i32,
                            );
                            cref::highbd_convolve_2d_scale_full(
                                &f.src,
                                f.origin,
                                f.stride,
                                &mut c_dst,
                                w,
                                &mut c_cb,
                                cb_stride,
                                w,
                                h,
                                fx_i,
                                w as i32,
                                fx_i,
                                h as i32,
                                cref::ScalePhases {
                                    subpel_x_qn: spx,
                                    x_step_qn: stx,
                                    subpel_y_qn: spy,
                                    y_step_qn: sty,
                                },
                                cref::RefCompound {
                                    is_compound,
                                    do_average,
                                    use_jnt,
                                    fwd,
                                    bck,
                                },
                                bd as i32,
                            );
                            assert_eq!(r_cb, c_cb, "hbd scale CONV_BUF bd{bd} {w}x{h} fx{fx_i}");
                            assert_eq!(r_dst, c_dst, "hbd scale dst bd{bd} {w}x{h} fx{fx_i}");
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 1400, "anti-vacuity: only {cells} cells ran");
}
