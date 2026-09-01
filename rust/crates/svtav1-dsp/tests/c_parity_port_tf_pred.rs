//! Differential parity for the temporal-filter motion compensation —
//! evidence tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbols driven (both `nm -g`-visible as `T`): `tf_inter_predictor`
//! (enc_inter_prediction.c:2452) and, through it,
//! `svt_aom_simple_luma_unipred` (:2677) — whose whole body is one
//! `tf_inter_predictor` call, so driving that call with the parameter set it
//! builds (identity scale, `is_compound = 0`, CONV_BUF at stride 128) covers
//! it completely.
//!
//! # Both arms, and why the 10-bit one needed a new binding
//!
//! `svtav1_cref::inter_pred::tf_inter_predictor` binds this C function through
//! `u8` slices. That expresses the 8-bit arm exactly, and cannot express the
//! 10-bit one: for `bit_depth > 8` C casts `src_ptr` to `uint16_t*` AND scales
//! the position offset by `1 << is_highbd` (:2478), so a `u8`-slice caller
//! would have to state the plane's start in BYTES and assume a byte order.
//! `svtav1_cref::interpred_gap::tf_inter_predictor_hbd` takes `u16` planes and
//! lets C do the cast.
//!
//! # bd 12 is EXCLUDED, and it is C's dispatch that excludes it
//!
//! MEASURED here 2026-08-31 on aarch64: at `bd = 12`, an 8x8 block at
//! `mv (3, 5)` (i.e. the 2-D kernel) comes back from C's dispatched
//! `svt_av1_highbd_convolve_2d_sr` as **all zeros**, while the port produces
//! real samples. Root: the NEON kernel derives its offsets from the
//! compile-time `ROUND0_BITS` rather than `conv_params->round_0`
//! (ASM_NEON/highbd_convolve_neon.c:1003-1006), and
//! `get_conv_params_no_round` sets `round_0 = 5` at bd 12 — the same defect
//! docs/SUSPECTED-C-BUGS.md #21 records for the `jnt_*` family, now measured
//! on the single-prediction `*_sr` family too. `mv (0, 0)` (the copy kernel,
//! which reads no rounding at all) agrees at bd 12.
//!
//! bd 12 is outside C's shipping envelope anyway (`svt_av1_verify_settings`,
//! enc_settings.c:460, accepts 8 and 10 only), so the sweep runs bd 10 — the
//! whole reachable high-bit-depth domain — rather than pinning a divergence
//! that only exists in an unreachable configuration.
//!
//! # `subsampling_shift`, the parameter that makes this function odd
//!
//! It is NOT a chroma flag. It shifts both STRIDES left and the block HEIGHT
//! right (:2481-2483) — the caller asking for every other row of a plane whose
//! rows are twice as far apart. `shift = 1` is therefore a real, separate
//! geometry, and [`subsampling_shift_is_covered`] asserts the sweep reaches
//! it rather than leaving it to inspection.

use svtav1_cref::inter_pred::{self as cref, RefMbEdges};
use svtav1_cref::interpred_gap as gap;
use svtav1_dsp::port_convolve::{ConvolveParams, InterpFilterKind};
use svtav1_dsp::port_inter_predictor::make_interp_filters;
use svtav1_dsp::port_scale_factors::ScaleFactors;
use svtav1_dsp::port_subpel_params::{MbEdges, Mv, RefGeometry};
use svtav1_dsp::port_tf_pred::{
    SIMPLE_UNIPRED_CONV_STRIDE, TfDst, TfSrc, simple_luma_unipred, tf_inter_predictor,
};

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

const BORDER: usize = 64;
const FRAME: i32 = 192;

fn plane8(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            let v = xs(&mut s);
            match v % 8 {
                0 => 0,
                1 => 255,
                _ => (v >> 13) as u8,
            }
        })
        .collect()
}

fn plane16(n: usize, seed: u32, bd: i32) -> Vec<u16> {
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

const MVS: [(i32, i32); 6] = [
    (0, 0),
    (3, 5),
    (-9, 4),
    (17, -33),
    (256, -256),
    (-1024, 1024),
];
const SIZES: [(usize, usize); 5] = [(8, 8), (16, 16), (32, 16), (16, 32), (64, 64)];
const SHIFTS: [u32; 2] = [0, 1];

fn edges_for(pre_x: i32, pre_y: i32, w: usize, h: usize) -> MbEdges {
    MbEdges {
        to_left: -(pre_x * 8),
        to_right: (FRAME - pre_x - w as i32) * 8,
        to_top: -(pre_y * 8),
        to_bottom: (FRAME - pre_y - h as i32) * 8,
    }
}

fn geom() -> RefGeometry {
    RefGeometry {
        super_block_size: 64,
        frame_width: FRAME,
        frame_height: FRAME,
    }
}

#[test]
fn tf_inter_predictor_8bit_matches_c() {
    let mut cells = 0usize;
    let stride = (FRAME as usize) + 2 * BORDER;
    for (fi, fk) in [
        (0usize, InterpFilterKind::EightTapRegular),
        (1, InterpFilterKind::EightTapSmooth),
        (2, InterpFilterKind::MultiTapSharp),
        (3, InterpFilterKind::Bilinear),
    ] {
        let filters = make_interp_filters(fk, fk);
        for &(w, h) in &SIZES {
            for &shift in &SHIFTS {
                for (mi, mv) in MVS.into_iter().enumerate() {
                    let (pre_x, pre_y) = (48i32, 48i32);
                    let src = plane8(stride * stride, 0x11a0_0001 + (mi + fi) as u32);
                    let edges = edges_for(pre_x, pre_y, w, h);
                    let dst_stride = w;
                    let mut r_dst = vec![0u8; dst_stride * h + 64];
                    let mut c_dst = vec![0u8; dst_stride * h + 64];
                    let mut cb =
                        vec![0u16; SIMPLE_UNIPRED_CONV_STRIDE * SIMPLE_UNIPRED_CONV_STRIDE];

                    tf_inter_predictor(
                        TfSrc::Lbd(&src),
                        0,
                        stride,
                        TfDst::Lbd(&mut r_dst),
                        dst_stride,
                        &mut cb,
                        pre_y,
                        pre_x,
                        Mv {
                            x: mv.0 as i16,
                            y: mv.1 as i16,
                        },
                        &ScaleFactors::setup_for_frame(FRAME, FRAME, FRAME, FRAME),
                        &ConvolveParams::no_round(false, SIMPLE_UNIPRED_CONV_STRIDE, false, 8),
                        filters,
                        geom(),
                        w,
                        h,
                        &edges,
                        8,
                        shift,
                    )
                    .expect("depths match");

                    let mut c_src = src.clone();
                    cref::tf_inter_predictor(
                        &mut c_src,
                        0,
                        stride,
                        &mut c_dst,
                        dst_stride,
                        (pre_y, pre_x),
                        mv,
                        (FRAME, FRAME, FRAME, FRAME),
                        64,
                        (FRAME, FRAME),
                        (w as i32, h as i32),
                        RefMbEdges {
                            to_left: edges.to_left,
                            to_right: edges.to_right,
                            to_top: edges.to_top,
                            to_bottom: edges.to_bottom,
                        },
                        filters,
                        8,
                        shift as i32,
                    );
                    assert_eq!(r_dst, c_dst, "tf 8bit {w}x{h} shift{shift} mv{mv:?} f{fi}");
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 240, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn tf_inter_predictor_hbd_matches_c() {
    let mut cells = 0usize;
    let stride = (FRAME as usize) + 2 * BORDER;
    for bd in [10] {
        for (fi, fk) in [
            (0usize, InterpFilterKind::EightTapRegular),
            (1, InterpFilterKind::EightTapSmooth),
            (2, InterpFilterKind::Bilinear),
        ] {
            let filters = make_interp_filters(fk, fk);
            for &(w, h) in &SIZES {
                for &shift in &SHIFTS {
                    for (mi, mv) in MVS.into_iter().enumerate() {
                        let (pre_x, pre_y) = (48i32, 48i32);
                        let src = plane16(
                            stride * stride,
                            0x22b0_0001 + (mi + fi) as u32 + bd as u32,
                            bd,
                        );
                        let edges = edges_for(pre_x, pre_y, w, h);
                        let dst_stride = w;
                        let mut r_dst = vec![0u16; dst_stride * h + 64];
                        let mut c_dst = vec![0u16; dst_stride * h + 64];
                        let mut r_cb =
                            vec![0u16; SIMPLE_UNIPRED_CONV_STRIDE * SIMPLE_UNIPRED_CONV_STRIDE];
                        let mut c_cb = r_cb.clone();

                        tf_inter_predictor(
                            TfSrc::Hbd(&src),
                            0,
                            stride,
                            TfDst::Hbd(&mut r_dst),
                            dst_stride,
                            &mut r_cb,
                            pre_y,
                            pre_x,
                            Mv {
                                x: mv.0 as i16,
                                y: mv.1 as i16,
                            },
                            &ScaleFactors::setup_for_frame(FRAME, FRAME, FRAME, FRAME),
                            &ConvolveParams::no_round(false, SIMPLE_UNIPRED_CONV_STRIDE, false, bd),
                            filters,
                            geom(),
                            w,
                            h,
                            &edges,
                            bd,
                            shift,
                        )
                        .expect("depths match");

                        let mut c_src = src.clone();
                        gap::tf_inter_predictor_hbd(
                            &mut c_src,
                            0,
                            stride,
                            &mut c_dst,
                            dst_stride,
                            &mut c_cb,
                            SIMPLE_UNIPRED_CONV_STRIDE,
                            (pre_y, pre_x),
                            mv,
                            (FRAME, FRAME, FRAME, FRAME),
                            64,
                            (FRAME, FRAME),
                            (w as i32, h as i32),
                            (edges.to_left, edges.to_right, edges.to_top, edges.to_bottom),
                            filters,
                            bd,
                            shift as i32,
                        );
                        assert_eq!(
                            r_dst, c_dst,
                            "tf hbd bd{bd} {w}x{h} shift{shift} mv{mv:?} f{fi}"
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 180, "anti-vacuity: only {cells} cells ran");
}

/// `svt_aom_simple_luma_unipred` builds a fixed parameter set and calls
/// `tf_inter_predictor`. This drives the port's wrapper and compares against C
/// through the same fixed set, including the `dst_origin_*` offset.
#[test]
fn simple_luma_unipred_matches_c() {
    let mut cells = 0usize;
    let stride = (FRAME as usize) + 2 * BORDER;
    let filters = make_interp_filters(
        InterpFilterKind::EightTapRegular,
        InterpFilterKind::EightTapSmooth,
    );
    for &(w, h) in &SIZES {
        for &shift in &SHIFTS {
            for (mi, mv) in MVS.into_iter().enumerate() {
                for (ox, oy) in [(0usize, 0usize), (8, 4)] {
                    let (pre_x, pre_y) = (48i32, 48i32);
                    let src = plane8(stride * stride, 0x33c0_0001 + mi as u32);
                    let edges = edges_for(pre_x, pre_y, w, h);
                    let dst_stride = w + 16;
                    let n = dst_stride * (h + 8);
                    let mut r_dst = vec![0u8; n];
                    let mut c_dst = vec![0u8; n];

                    simple_luma_unipred(
                        TfSrc::Lbd(&src),
                        stride,
                        TfDst::Lbd(&mut r_dst),
                        dst_stride,
                        ox,
                        oy,
                        Mv {
                            x: mv.0 as i16,
                            y: mv.1 as i16,
                        },
                        filters,
                        geom(),
                        FRAME,
                        FRAME,
                        pre_x,
                        pre_y,
                        w,
                        h,
                        &edges,
                        8,
                        shift,
                    )
                    .expect("depths match");

                    let mut c_src = src.clone();
                    let off = ox + oy * dst_stride;
                    cref::tf_inter_predictor(
                        &mut c_src,
                        0,
                        stride,
                        &mut c_dst[off..],
                        dst_stride,
                        (pre_y, pre_x),
                        mv,
                        (FRAME, FRAME, FRAME, FRAME),
                        64,
                        (FRAME, FRAME),
                        (w as i32, h as i32),
                        RefMbEdges {
                            to_left: edges.to_left,
                            to_right: edges.to_right,
                            to_top: edges.to_top,
                            to_bottom: edges.to_bottom,
                        },
                        filters,
                        8,
                        shift as i32,
                    );
                    assert_eq!(
                        r_dst, c_dst,
                        "simple_luma_unipred {w}x{h} shift{shift} mv{mv:?} origin({ox},{oy})"
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 120, "anti-vacuity: only {cells} cells ran");
}

/// POSITIVE CONTROL: `subsampling_shift = 1` must actually change the output,
/// otherwise the sweeps above prove nothing about it (a port that ignored the
/// parameter would pass every cell where C also ignored it).
#[test]
fn subsampling_shift_is_covered() {
    let stride = (FRAME as usize) + 2 * BORDER;
    let (w, h) = (16usize, 16usize);
    let (pre_x, pre_y) = (48i32, 48i32);
    let src = plane8(stride * stride, 0x44d0_0001);
    let edges = edges_for(pre_x, pre_y, w, h);
    let filters = make_interp_filters(
        InterpFilterKind::EightTapRegular,
        InterpFilterKind::EightTapRegular,
    );
    let mut out = Vec::new();
    for shift in SHIFTS {
        let mut d = vec![0u8; w * h];
        let mut cb = vec![0u16; SIMPLE_UNIPRED_CONV_STRIDE * SIMPLE_UNIPRED_CONV_STRIDE];
        tf_inter_predictor(
            TfSrc::Lbd(&src),
            0,
            stride,
            TfDst::Lbd(&mut d),
            w,
            &mut cb,
            pre_y,
            pre_x,
            Mv { x: 5, y: -7 },
            &ScaleFactors::setup_for_frame(FRAME, FRAME, FRAME, FRAME),
            &ConvolveParams::no_round(false, SIMPLE_UNIPRED_CONV_STRIDE, false, 8),
            filters,
            geom(),
            w,
            h,
            &edges,
            8,
            shift,
        )
        .expect("depths match");
        out.push(d);
    }
    assert_ne!(
        out[0], out[1],
        "shift 0 and shift 1 produced the same pixels: the parameter is inert here, \
         so every cell above that varies it proves nothing"
    );
}

/// A depth-mismatched pair is REFUSED, not silently reinterpreted.
#[test]
fn mixed_depths_are_refused() {
    let src = plane8(64, 1);
    let mut dst = vec![0u16; 64];
    let mut cb = vec![0u16; 128 * 128];
    let err = tf_inter_predictor(
        TfSrc::Lbd(&src),
        0,
        8,
        TfDst::Hbd(&mut dst),
        8,
        &mut cb,
        0,
        0,
        Mv { x: 0, y: 0 },
        &ScaleFactors::setup_for_frame(64, 64, 64, 64),
        &ConvolveParams::no_round(false, 128, false, 8),
        0,
        RefGeometry {
            super_block_size: 64,
            frame_width: 64,
            frame_height: 64,
        },
        8,
        8,
        &MbEdges {
            to_left: 0,
            to_right: 0,
            to_top: 0,
            to_bottom: 0,
        },
        8,
        0,
    );
    assert!(err.is_err());
    assert_eq!(dst, vec![0u16; 64], "a refused call wrote pixels");
}
