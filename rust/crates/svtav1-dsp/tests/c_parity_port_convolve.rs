//! Differential parity for the AV1 inter *reconstruction* MC kernels against
//! the real exported C symbols — evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven (all `nm -g`-visible in `Bin/Release/libSvtAv1Enc.a`):
//! `svt_av1_convolve_2d_sr_c`, `svt_av1_convolve_x_sr_c`,
//! `svt_av1_convolve_y_sr_c`, `svt_av1_convolve_2d_copy_sr_c`,
//! `svt_av1_jnt_convolve_2d_c`, `svt_av1_jnt_convolve_x_c`,
//! `svt_av1_jnt_convolve_y_c`, `svt_av1_jnt_convolve_2d_copy_c`,
//! `svt_av1_setup_scale_factors_for_frame`,
//! `svt_av1_dist_wtd_comp_weight_assign`, plus the header-inline filter-param
//! selection and `get_conv_params_no_round` rounding derivation via shims.
//!
//! These are NOT the kernels `c_parity_inter_pred.rs` covers — that file gates
//! `svt_aom_convolve8_{horiz,vert}_c`, the single-pass ME upsample kernels.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_convolve::{
    ConvolveParams, FilterParams, InterpFilterKind, SrcView, convolve_2d_copy_sr, convolve_2d_sr,
    convolve_x_sr, convolve_y_sr, interp_filter_params_with_block_size, jnt_convolve_2d,
    jnt_convolve_2d_copy, jnt_convolve_x, jnt_convolve_y,
};

/// Deterministic pseudo-random plane. A xorshift keeps the fixture in the file
/// (no corpus dependency, so it can never SKIP-MISSING).
fn plane(w: usize, h: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..w * h)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s >> 3) as u8
        })
        .collect()
}

/// Margin (in pixels) around the block: 3 taps precede the origin and 4 follow.
const PAD: usize = 8;

struct Fixture {
    src: Vec<u8>,
    stride: usize,
    origin: usize,
}

impl Fixture {
    fn new(w: usize, h: usize, seed: u32) -> Self {
        let stride = w + 2 * PAD;
        let rows = h + 2 * PAD;
        Self {
            src: plane(stride, rows, seed),
            stride,
            origin: PAD * stride + PAD,
        }
    }
    fn view(&self) -> SrcView<'_> {
        SrcView::new(&self.src, self.origin, self.stride)
    }
}

const FILTERS: [(i32, InterpFilterKind); 4] = [
    (0, InterpFilterKind::EightTapRegular),
    (1, InterpFilterKind::EightTapSmooth),
    (2, InterpFilterKind::MultiTapSharp),
    (3, InterpFilterKind::Bilinear),
];

/// Block sizes that exercise both the >4 and the <=4 (narrow-block 4-tap
/// substitution) arms of `av1_get_interp_filter_params_with_block_size`.
const SIZES: [(usize, usize); 8] = [
    (4, 4),
    (4, 8),
    (8, 4),
    (8, 8),
    (16, 8),
    (16, 16),
    (32, 16),
    (64, 64),
];

/// The Rust filter table + narrow-block selection rule must equal the C
/// tables, phase by phase, for every filter and both the wide and narrow arms.
#[test]
fn filter_params_match_c() {
    for (c_idx, kind) in FILTERS {
        for size in [2, 4, 8, 16, 64] {
            let rust = interp_filter_params_with_block_size(kind, size);
            assert_eq!(
                cref::interp_filter_taps(c_idx, size),
                8,
                "taps for filt {c_idx} size {size}"
            );
            for subpel in 0..16 {
                assert_eq!(
                    *rust.subpel_kernel(subpel),
                    cref::interp_filter_kernel(c_idx, size, subpel),
                    "filt {c_idx} size {size} phase {subpel}"
                );
            }
        }
    }
}

/// `get_conv_params_no_round`'s round_0 / round_1 derivation, including the
/// `intbufrange > 16` correction that only fires at bd 12.
#[test]
fn conv_params_rounds_match_c() {
    for bd in [8, 10, 12] {
        for is_compound in [false, true] {
            let rust = ConvolveParams::no_round(false, 64, is_compound, bd);
            assert_eq!(
                (rust.round_0, rust.round_1),
                cref::conv_params_rounds(false, 64, is_compound, bd),
                "bd {bd} compound {is_compound}"
            );
        }
    }
}

fn params_for(kind: InterpFilterKind, size: usize) -> FilterParams {
    interp_filter_params_with_block_size(kind, size as i32)
}

#[test]
fn convolve_2d_sr_matches_c() {
    let cp = ConvolveParams::single(false, 8);
    let mut cells = 0usize;
    for (fx_i, fx_k) in FILTERS {
        for (fy_i, fy_k) in FILTERS {
            for (w, h) in SIZES {
                for (sx, sy) in [(1, 1), (7, 3), (8, 8), (15, 15), (3, 12)] {
                    let f = Fixture::new(w, h, 0x1234_5678 ^ (w as u32) << 8 ^ sx as u32);
                    let mut got = vec![0u8; w * h];
                    convolve_2d_sr(
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
                    );
                    let mut want = vec![0u8; w * h];
                    cref::convolve_2d_sr(
                        &f.src, f.origin, f.stride, &mut want, w, w, h, fx_i, w as i32, fy_i,
                        h as i32, sx, sy,
                    );
                    assert_eq!(got, want, "2d_sr {w}x{h} fx{fx_i} fy{fy_i} sub({sx},{sy})");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 640, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn convolve_x_sr_matches_c() {
    let cp = ConvolveParams::single(false, 8);
    let mut cells = 0usize;
    for (fx_i, fx_k) in FILTERS {
        for (w, h) in SIZES {
            for sx in [0, 1, 5, 8, 15] {
                let f = Fixture::new(w, h, 0x0BAD_F00D ^ (h as u32) << 4 ^ sx as u32);
                let mut got = vec![0u8; w * h];
                convolve_x_sr(f.view(), &mut got, w, w, h, &params_for(fx_k, w), sx, &cp);
                let mut want = vec![0u8; w * h];
                cref::convolve_x_sr(
                    &f.src, f.origin, f.stride, &mut want, w, w, h, fx_i, w as i32, sx,
                );
                assert_eq!(got, want, "x_sr {w}x{h} fx{fx_i} sub {sx}");
                cells += 1;
            }
        }
    }
    assert!(cells >= 160, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn convolve_y_sr_matches_c() {
    let mut cells = 0usize;
    for (fy_i, fy_k) in FILTERS {
        for (w, h) in SIZES {
            for sy in [0, 2, 6, 8, 15] {
                let f = Fixture::new(w, h, 0xDEAD_BEEF ^ (w as u32) << 2 ^ sy as u32);
                let mut got = vec![0u8; w * h];
                convolve_y_sr(f.view(), &mut got, w, w, h, &params_for(fy_k, h), sy);
                let mut want = vec![0u8; w * h];
                cref::convolve_y_sr(
                    &f.src, f.origin, f.stride, &mut want, w, w, h, fy_i, h as i32, sy,
                );
                assert_eq!(got, want, "y_sr {w}x{h} fy{fy_i} sub {sy}");
                cells += 1;
            }
        }
    }
    assert!(cells >= 160, "anti-vacuity: only {cells} cells ran");
}

/// The PD0 kernel: `svt_inter_predictor_pd0` always passes subpel (0,0), so
/// this is the entirety of PD0's MC surface for single prediction.
#[test]
fn convolve_2d_copy_sr_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for dst_stride in [w, w + 7] {
            let f = Fixture::new(w, h, 0xC0FF_EE01 ^ (h as u32));
            let mut got = vec![0u8; dst_stride * h];
            convolve_2d_copy_sr(f.view(), &mut got, dst_stride, w, h);
            let mut want = vec![0u8; dst_stride * h];
            cref::convolve_2d_copy_sr(&f.src, f.origin, f.stride, &mut want, dst_stride, w, h);
            assert_eq!(got, want, "2d_copy_sr {w}x{h} dst_stride {dst_stride}");
            cells += 1;
        }
    }
    assert!(cells >= 16, "anti-vacuity: only {cells} cells ran");
}

/// Compound configurations: the first pass writes the CONV_BUF
/// (`do_average = 0`), the second reads it back with plain averaging or with
/// the distance weights `svt_av1_dist_wtd_comp_weight_assign` produces.
const JNT_WEIGHTS: [(bool, i32, i32); 4] =
    [(false, 0, 0), (true, 9, 7), (true, 4, 12), (true, 13, 3)];

#[test]
fn jnt_convolve_2d_matches_c() {
    let mut cells = 0usize;
    for (fx_i, fx_k) in FILTERS {
        for (fy_i, fy_k) in FILTERS {
            for (w, h) in SIZES {
                for (sx, sy) in [(1, 1), (9, 4), (15, 8)] {
                    for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                        let cb_stride = w + 3;
                        let f0 = Fixture::new(w, h, 0x5EED_0001 ^ (w as u32) << 6 ^ sx as u32);
                        let f1 = Fixture::new(w, h, 0x5EED_0002 ^ (h as u32) << 6 ^ sy as u32);

                        // Rust: pass 0 fills the CONV_BUF, pass 1 averages.
                        let mut r_cb = vec![0u16; cb_stride * h];
                        let mut r_dst = vec![0u8; w * h];
                        let mut cp = ConvolveParams::no_round(false, cb_stride, true, 8);
                        cp.use_jnt_comp_avg = use_jnt;
                        cp.fwd_offset = fwd;
                        cp.bck_offset = bck;
                        jnt_convolve_2d(
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
                        );
                        cp.do_average = true;
                        jnt_convolve_2d(
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
                        );

                        let mut c_cb = vec![0u16; cb_stride * h];
                        let mut c_dst = vec![0u8; w * h];
                        cref::jnt_convolve_2d(
                            &f0.src,
                            f0.origin,
                            f0.stride,
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
                            cref::JntCfg {
                                do_average: false,
                                use_jnt,
                                fwd,
                                bck,
                            },
                        );
                        cref::jnt_convolve_2d(
                            &f1.src,
                            f1.origin,
                            f1.stride,
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
                            cref::JntCfg {
                                do_average: true,
                                use_jnt,
                                fwd,
                                bck,
                            },
                        );

                        assert_eq!(
                            r_cb, c_cb,
                            "jnt_2d CONV_BUF {w}x{h} fx{fx_i} fy{fy_i} sub({sx},{sy}) jnt{use_jnt}"
                        );
                        assert_eq!(
                            r_dst, c_dst,
                            "jnt_2d dst {w}x{h} fx{fx_i} fy{fy_i} sub({sx},{sy}) jnt{use_jnt}"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 1500, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn jnt_convolve_x_matches_c() {
    let mut cells = 0usize;
    for (fx_i, fx_k) in FILTERS {
        for (w, h) in SIZES {
            for sx in [0, 3, 15] {
                for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                    let cb_stride = w + 1;
                    let f0 = Fixture::new(w, h, 0x11_1111 ^ (w as u32) ^ sx as u32);
                    let f1 = Fixture::new(w, h, 0x22_2222 ^ (h as u32) ^ sx as u32);

                    let mut r_cb = vec![0u16; cb_stride * h];
                    let mut r_dst = vec![0u8; w * h];
                    let mut cp = ConvolveParams::no_round(false, cb_stride, true, 8);
                    cp.use_jnt_comp_avg = use_jnt;
                    cp.fwd_offset = fwd;
                    cp.bck_offset = bck;
                    jnt_convolve_x(
                        f0.view(),
                        &mut r_dst,
                        w,
                        &mut r_cb,
                        w,
                        h,
                        &params_for(fx_k, w),
                        sx,
                        &cp,
                    );
                    cp.do_average = true;
                    jnt_convolve_x(
                        f1.view(),
                        &mut r_dst,
                        w,
                        &mut r_cb,
                        w,
                        h,
                        &params_for(fx_k, w),
                        sx,
                        &cp,
                    );

                    let mut c_cb = vec![0u16; cb_stride * h];
                    let mut c_dst = vec![0u8; w * h];
                    for (fx, avg) in [(&f0, false), (&f1, true)] {
                        cref::jnt_convolve_x(
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
                            cref::JntCfg {
                                do_average: avg,
                                use_jnt,
                                fwd,
                                bck,
                            },
                        );
                    }
                    assert_eq!(r_cb, c_cb, "jnt_x CONV_BUF {w}x{h} fx{fx_i} sub {sx}");
                    assert_eq!(r_dst, c_dst, "jnt_x dst {w}x{h} fx{fx_i} sub {sx}");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 380, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn jnt_convolve_y_matches_c() {
    let mut cells = 0usize;
    for (fy_i, fy_k) in FILTERS {
        for (w, h) in SIZES {
            for sy in [0, 6, 15] {
                for (use_jnt, fwd, bck) in JNT_WEIGHTS {
                    let cb_stride = w + 5;
                    let f0 = Fixture::new(w, h, 0x33_3333 ^ (w as u32) ^ sy as u32);
                    let f1 = Fixture::new(w, h, 0x44_4444 ^ (h as u32) ^ sy as u32);

                    let mut r_cb = vec![0u16; cb_stride * h];
                    let mut r_dst = vec![0u8; w * h];
                    let mut cp = ConvolveParams::no_round(false, cb_stride, true, 8);
                    cp.use_jnt_comp_avg = use_jnt;
                    cp.fwd_offset = fwd;
                    cp.bck_offset = bck;
                    jnt_convolve_y(
                        f0.view(),
                        &mut r_dst,
                        w,
                        &mut r_cb,
                        w,
                        h,
                        &params_for(fy_k, h),
                        sy,
                        &cp,
                    );
                    cp.do_average = true;
                    jnt_convolve_y(
                        f1.view(),
                        &mut r_dst,
                        w,
                        &mut r_cb,
                        w,
                        h,
                        &params_for(fy_k, h),
                        sy,
                        &cp,
                    );

                    let mut c_cb = vec![0u16; cb_stride * h];
                    let mut c_dst = vec![0u8; w * h];
                    for (fx, avg) in [(&f0, false), (&f1, true)] {
                        cref::jnt_convolve_y(
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
                            cref::JntCfg {
                                do_average: avg,
                                use_jnt,
                                fwd,
                                bck,
                            },
                        );
                    }
                    assert_eq!(r_cb, c_cb, "jnt_y CONV_BUF {w}x{h} fy{fy_i} sub {sy}");
                    assert_eq!(r_dst, c_dst, "jnt_y dst {w}x{h} fy{fy_i} sub {sy}");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 380, "anti-vacuity: only {cells} cells ran");
}

/// The compound whole-pel kernel — the compound counterpart of the PD0 path.
/// Its `ConvBufType` (u16) arithmetic is the wrap this cell would catch.
#[test]
fn jnt_convolve_2d_copy_matches_c() {
    let mut cells = 0usize;
    for (w, h) in SIZES {
        for (use_jnt, fwd, bck) in JNT_WEIGHTS {
            let cb_stride = w + 2;
            let f0 = Fixture::new(w, h, 0x55_5555 ^ (w as u32));
            let f1 = Fixture::new(w, h, 0x66_6666 ^ (h as u32));

            let mut r_cb = vec![0u16; cb_stride * h];
            let mut r_dst = vec![0u8; w * h];
            let mut cp = ConvolveParams::no_round(false, cb_stride, true, 8);
            cp.use_jnt_comp_avg = use_jnt;
            cp.fwd_offset = fwd;
            cp.bck_offset = bck;
            jnt_convolve_2d_copy(f0.view(), &mut r_dst, w, &mut r_cb, w, h, &cp);
            cp.do_average = true;
            jnt_convolve_2d_copy(f1.view(), &mut r_dst, w, &mut r_cb, w, h, &cp);

            let mut c_cb = vec![0u16; cb_stride * h];
            let mut c_dst = vec![0u8; w * h];
            for (fx, avg) in [(&f0, false), (&f1, true)] {
                cref::jnt_convolve_2d_copy(
                    &fx.src,
                    fx.origin,
                    fx.stride,
                    &mut c_dst,
                    w,
                    &mut c_cb,
                    cb_stride,
                    w,
                    h,
                    cref::JntCfg {
                        do_average: avg,
                        use_jnt,
                        fwd,
                        bck,
                    },
                );
            }
            assert_eq!(r_cb, c_cb, "jnt_2d_copy CONV_BUF {w}x{h} jnt{use_jnt}");
            assert_eq!(r_dst, c_dst, "jnt_2d_copy dst {w}x{h} jnt{use_jnt}");
            cells += 1;
        }
    }
    assert!(cells >= 32, "anti-vacuity: only {cells} cells ran");
}
