//! Differential parity for the 10/12-bit inter reconstruction MC kernels —
//! evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven (all `nm -g`-visible in `Bin/Release/libSvtAv1Enc.a`):
//! `svt_av1_highbd_convolve_2d_sr_c`, `svt_av1_highbd_convolve_x_sr_c`,
//! `svt_av1_highbd_convolve_y_sr_c`, `svt_av1_highbd_convolve_2d_copy_sr_c`,
//! `svt_av1_highbd_jnt_convolve_2d_c`, `svt_av1_highbd_jnt_convolve_x_c`,
//! `svt_av1_highbd_jnt_convolve_y_c`, `svt_av1_highbd_jnt_convolve_2d_copy_c`.
//!
//! Bit depths 10 and 12 are both swept: 12 is where
//! `get_conv_params_no_round`'s `intbufrange > 16` correction fires, moving
//! `round_0` 3 -> 5, and it moves the offset arithmetic in every kernel.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_convolve::{
    ConvolveParams, FilterParams, InterpFilterKind, interp_filter_params_with_block_size,
};
use svtav1_dsp::port_convolve_hbd::{
    SrcView16, highbd_convolve_2d_copy_sr, highbd_convolve_2d_sr, highbd_convolve_x_sr,
    highbd_convolve_y_sr, highbd_jnt_convolve_2d, highbd_jnt_convolve_2d_copy,
    highbd_jnt_convolve_x, highbd_jnt_convolve_y,
};

const PAD: usize = 8;

struct Fixture {
    src: Vec<u16>,
    stride: usize,
    origin: usize,
}

impl Fixture {
    fn new(w: usize, h: usize, seed: u32, bd: i32) -> Self {
        let stride = w + 2 * PAD;
        let rows = h + 2 * PAD;
        let max = (1u32 << bd) - 1;
        let mut s = seed | 1;
        let src = (0..stride * rows)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                // Bias hard toward both rails so the clip and the CONV_BUF
                // wraps are actually exercised, not just mid-range values.
                match s % 8 {
                    0 => 0,
                    1 => max as u16,
                    _ => ((s >> 5) % (max + 1)) as u16,
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

const SIZES: [(usize, usize); 6] = [(4, 4), (4, 8), (8, 4), (8, 8), (16, 16), (32, 32)];
const DEPTHS: [i32; 2] = [10, 12];
const JNT_WEIGHTS: [(bool, i32, i32); 3] = [(false, 0, 0), (true, 9, 7), (true, 4, 12)];

fn params_for(kind: InterpFilterKind, size: usize) -> FilterParams {
    interp_filter_params_with_block_size(kind, size as i32)
}

#[test]
fn highbd_convolve_2d_sr_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        let cp = ConvolveParams::single(false, bd);
        for (fx_i, fx_k) in FILTERS {
            for (fy_i, fy_k) in FILTERS {
                for (w, h) in SIZES {
                    for (sx, sy) in [(1, 1), (7, 3), (8, 8), (15, 15)] {
                        let f = Fixture::new(w, h, 0x1357_9BDF ^ (w as u32) << 8 ^ sx as u32, bd);
                        let mut got = vec![0u16; w * h];
                        highbd_convolve_2d_sr(
                            f.view(),
                            &mut got,
                            w,
                            w,
                            h,
                            &params_for(fx_k, w),
                            &params_for(fy_k, h),
                            sx,
                            sy,
                            &cp,
                            bd,
                        );
                        let mut want = vec![0u16; w * h];
                        cref::highbd_convolve_2d_sr(
                            &f.src, f.origin, f.stride, &mut want, w, w, h, fx_i, w as i32, fy_i,
                            h as i32, sx, sy, bd,
                        );
                        assert_eq!(
                            got, want,
                            "hbd 2d_sr bd{bd} {w}x{h} fx{fx_i} fy{fy_i} sub({sx},{sy})"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 768, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_convolve_x_sr_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        let cp = ConvolveParams::single(false, bd);
        for (fx_i, fx_k) in FILTERS {
            for (w, h) in SIZES {
                for sx in [0, 1, 8, 15] {
                    let f = Fixture::new(w, h, 0x2468_ACE0 ^ (h as u32) << 4 ^ sx as u32, bd);
                    let mut got = vec![0u16; w * h];
                    highbd_convolve_x_sr(
                        f.view(),
                        &mut got,
                        w,
                        w,
                        h,
                        &params_for(fx_k, w),
                        sx,
                        &cp,
                        bd,
                    );
                    let mut want = vec![0u16; w * h];
                    cref::highbd_convolve_x_sr(
                        &f.src, f.origin, f.stride, &mut want, w, w, h, fx_i, w as i32, sx, bd,
                    );
                    assert_eq!(got, want, "hbd x_sr bd{bd} {w}x{h} fx{fx_i} sub {sx}");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 192, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_convolve_y_sr_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        for (fy_i, fy_k) in FILTERS {
            for (w, h) in SIZES {
                for sy in [0, 2, 8, 15] {
                    let f = Fixture::new(w, h, 0x0FED_CBA9 ^ (w as u32) << 2 ^ sy as u32, bd);
                    let mut got = vec![0u16; w * h];
                    highbd_convolve_y_sr(f.view(), &mut got, w, w, h, &params_for(fy_k, h), sy, bd);
                    let mut want = vec![0u16; w * h];
                    cref::highbd_convolve_y_sr(
                        &f.src, f.origin, f.stride, &mut want, w, w, h, fy_i, h as i32, sy, bd,
                    );
                    assert_eq!(got, want, "hbd y_sr bd{bd} {w}x{h} fy{fy_i} sub {sy}");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 192, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_convolve_2d_copy_sr_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        for (w, h) in SIZES {
            for dst_stride in [w, w + 5] {
                let f = Fixture::new(w, h, 0x7777_0001 ^ (h as u32), bd);
                let mut got = vec![0u16; dst_stride * h];
                highbd_convolve_2d_copy_sr(f.view(), &mut got, dst_stride, w, h);
                let mut want = vec![0u16; dst_stride * h];
                cref::highbd_convolve_2d_copy_sr(
                    &f.src, f.origin, f.stride, &mut want, dst_stride, w, h, bd,
                );
                assert_eq!(
                    got, want,
                    "hbd 2d_copy_sr bd{bd} {w}x{h} stride {dst_stride}"
                );
                cells += 1;
            }
        }
    }
    assert!(cells >= 24, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_jnt_convolve_2d_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        for (fx_i, fx_k) in FILTERS {
            for (fy_i, fy_k) in FILTERS {
                for (w, h) in SIZES {
                    for (sx, sy) in [(1, 1), (15, 8)] {
                        for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                            let cb_stride = w + 3;
                            let f0 =
                                Fixture::new(w, h, 0xAAA0_0001 ^ (w as u32) << 6 ^ sx as u32, bd);
                            let f1 =
                                Fixture::new(w, h, 0xBBB0_0002 ^ (h as u32) << 6 ^ sy as u32, bd);

                            let mut r_cb = vec![0u16; cb_stride * h];
                            let mut r_dst = vec![0u16; w * h];
                            let mut cp = ConvolveParams::no_round(false, cb_stride, true, bd);
                            cp.use_jnt_comp_avg = use_jnt;
                            cp.fwd_offset = fwd;
                            cp.bck_offset = bck;
                            highbd_jnt_convolve_2d(
                                f0.view(),
                                &mut r_dst,
                                w,
                                &mut r_cb,
                                w,
                                h,
                                &params_for(fx_k, w),
                                &params_for(fy_k, h),
                                sx,
                                sy,
                                &cp,
                                bd,
                            );
                            cp.do_average = true;
                            highbd_jnt_convolve_2d(
                                f1.view(),
                                &mut r_dst,
                                w,
                                &mut r_cb,
                                w,
                                h,
                                &params_for(fx_k, w),
                                &params_for(fy_k, h),
                                sx,
                                sy,
                                &cp,
                                bd,
                            );

                            let mut c_cb = vec![0u16; cb_stride * h];
                            let mut c_dst = vec![0u16; w * h];
                            for (fx, avg) in [(&f0, false), (&f1, true)] {
                                cref::highbd_jnt_convolve_2d(
                                    &fx.src,
                                    fx.origin,
                                    fx.stride,
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
                                    sx,
                                    sy,
                                    bd,
                                    cref::JntCfg {
                                        do_average: avg,
                                        use_jnt,
                                        fwd,
                                        bck,
                                    },
                                );
                            }
                            assert_eq!(
                                r_cb, c_cb,
                                "hbd jnt_2d CONV_BUF bd{bd} {w}x{h} fx{fx_i} fy{fy_i}"
                            );
                            assert_eq!(
                                r_dst, c_dst,
                                "hbd jnt_2d dst bd{bd} {w}x{h} fx{fx_i} fy{fy_i}"
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 1100, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_jnt_convolve_x_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        for (fx_i, fx_k) in FILTERS {
            for (w, h) in SIZES {
                for sx in [0, 3, 15] {
                    for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                        let cb_stride = w + 1;
                        let f0 = Fixture::new(w, h, 0xC1C1_0001 ^ (w as u32) ^ sx as u32, bd);
                        let f1 = Fixture::new(w, h, 0xD2D2_0002 ^ (h as u32) ^ sx as u32, bd);

                        let mut r_cb = vec![0u16; cb_stride * h];
                        let mut r_dst = vec![0u16; w * h];
                        let mut cp = ConvolveParams::no_round(false, cb_stride, true, bd);
                        cp.use_jnt_comp_avg = use_jnt;
                        cp.fwd_offset = fwd;
                        cp.bck_offset = bck;
                        highbd_jnt_convolve_x(
                            f0.view(),
                            &mut r_dst,
                            w,
                            &mut r_cb,
                            w,
                            h,
                            &params_for(fx_k, w),
                            sx,
                            &cp,
                            bd,
                        );
                        cp.do_average = true;
                        highbd_jnt_convolve_x(
                            f1.view(),
                            &mut r_dst,
                            w,
                            &mut r_cb,
                            w,
                            h,
                            &params_for(fx_k, w),
                            sx,
                            &cp,
                            bd,
                        );

                        let mut c_cb = vec![0u16; cb_stride * h];
                        let mut c_dst = vec![0u16; w * h];
                        for (fx, avg) in [(&f0, false), (&f1, true)] {
                            cref::highbd_jnt_convolve_x(
                                &fx.src,
                                fx.origin,
                                fx.stride,
                                &mut c_dst,
                                w,
                                &mut c_cb,
                                cb_stride,
                                w,
                                h,
                                fx_i,
                                w as i32,
                                sx,
                                bd,
                                cref::JntCfg {
                                    do_average: avg,
                                    use_jnt,
                                    fwd,
                                    bck,
                                },
                            );
                        }
                        assert_eq!(
                            r_cb, c_cb,
                            "hbd jnt_x CONV_BUF bd{bd} {w}x{h} fx{fx_i} sub {sx}"
                        );
                        assert_eq!(
                            r_dst, c_dst,
                            "hbd jnt_x dst bd{bd} {w}x{h} fx{fx_i} sub {sx}"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 288, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_jnt_convolve_y_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        for (fy_i, fy_k) in FILTERS {
            for (w, h) in SIZES {
                for sy in [0, 6, 15] {
                    for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                        let cb_stride = w + 4;
                        let f0 = Fixture::new(w, h, 0xE3E3_0001 ^ (w as u32) ^ sy as u32, bd);
                        let f1 = Fixture::new(w, h, 0xF4F4_0002 ^ (h as u32) ^ sy as u32, bd);

                        let mut r_cb = vec![0u16; cb_stride * h];
                        let mut r_dst = vec![0u16; w * h];
                        let mut cp = ConvolveParams::no_round(false, cb_stride, true, bd);
                        cp.use_jnt_comp_avg = use_jnt;
                        cp.fwd_offset = fwd;
                        cp.bck_offset = bck;
                        highbd_jnt_convolve_y(
                            f0.view(),
                            &mut r_dst,
                            w,
                            &mut r_cb,
                            w,
                            h,
                            &params_for(fy_k, h),
                            sy,
                            &cp,
                            bd,
                        );
                        cp.do_average = true;
                        highbd_jnt_convolve_y(
                            f1.view(),
                            &mut r_dst,
                            w,
                            &mut r_cb,
                            w,
                            h,
                            &params_for(fy_k, h),
                            sy,
                            &cp,
                            bd,
                        );

                        let mut c_cb = vec![0u16; cb_stride * h];
                        let mut c_dst = vec![0u16; w * h];
                        for (fx, avg) in [(&f0, false), (&f1, true)] {
                            cref::highbd_jnt_convolve_y(
                                &fx.src,
                                fx.origin,
                                fx.stride,
                                &mut c_dst,
                                w,
                                &mut c_cb,
                                cb_stride,
                                w,
                                h,
                                fy_i,
                                h as i32,
                                sy,
                                bd,
                                cref::JntCfg {
                                    do_average: avg,
                                    use_jnt,
                                    fwd,
                                    bck,
                                },
                            );
                        }
                        assert_eq!(
                            r_cb, c_cb,
                            "hbd jnt_y CONV_BUF bd{bd} {w}x{h} fy{fy_i} sub {sy}"
                        );
                        assert_eq!(
                            r_dst, c_dst,
                            "hbd jnt_y dst bd{bd} {w}x{h} fy{fy_i} sub {sy}"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 288, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn highbd_jnt_convolve_2d_copy_matches_c() {
    let mut cells = 0usize;
    for bd in DEPTHS {
        for (w, h) in SIZES {
            for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                let cb_stride = w + 2;
                let f0 = Fixture::new(w, h, 0x9090_0001 ^ (w as u32), bd);
                let f1 = Fixture::new(w, h, 0x8080_0002 ^ (h as u32), bd);

                let mut r_cb = vec![0u16; cb_stride * h];
                let mut r_dst = vec![0u16; w * h];
                let mut cp = ConvolveParams::no_round(false, cb_stride, true, bd);
                cp.use_jnt_comp_avg = use_jnt;
                cp.fwd_offset = fwd;
                cp.bck_offset = bck;
                highbd_jnt_convolve_2d_copy(f0.view(), &mut r_dst, w, &mut r_cb, w, h, &cp, bd);
                cp.do_average = true;
                highbd_jnt_convolve_2d_copy(f1.view(), &mut r_dst, w, &mut r_cb, w, h, &cp, bd);

                let mut c_cb = vec![0u16; cb_stride * h];
                let mut c_dst = vec![0u16; w * h];
                for (fx, avg) in [(&f0, false), (&f1, true)] {
                    cref::highbd_jnt_convolve_2d_copy(
                        &fx.src,
                        fx.origin,
                        fx.stride,
                        &mut c_dst,
                        w,
                        &mut c_cb,
                        cb_stride,
                        w,
                        h,
                        bd,
                        cref::JntCfg {
                            do_average: avg,
                            use_jnt,
                            fwd,
                            bck,
                        },
                    );
                }
                assert_eq!(r_cb, c_cb, "hbd jnt_2d_copy CONV_BUF bd{bd} {w}x{h}");
                assert_eq!(r_dst, c_dst, "hbd jnt_2d_copy dst bd{bd} {w}x{h}");
                cells += 1;
            }
        }
    }
    assert!(cells >= 36, "anti-vacuity: only {cells} cells ran");
}
