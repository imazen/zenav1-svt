//! Differential parity for the OBMC neighbour's own prediction — evidence
//! tier 1 for the call, tier 4 for the wrapper (`WORKING-ON-THIS.md` §4).
//!
//! The four C functions (`get_single_prediction_for_obmc_luma` :958,
//! `..._chroma` :1018, and their `_hbd` twins :791 / :853) are `static`, so
//! there is no symbol to bind. Each one's ENTIRE BODY is a fixed parameter set
//! plus one `svt_aom_enc_make_inter_predictor` call (two for the chroma pair),
//! and that IS exported — so this drives C with exactly the parameters the
//! port derives. A wrong constant in the port's parameter set fails here; only
//! the four-line wrapping around it is untested by a symbol.
//!
//! # What each cell varies, and why
//!
//! * All three source representations, because the 8-bit pair and the `_hbd`
//!   pair differ precisely in that (`is16bit` + `y_buffer_bit_inc`).
//! * `ss_x` / `ss_y` on the chroma calls, including the ASYMMETRIC pairs
//!   `(1, 0)` and `(0, 1)` that 4:2:0 never produces. They are the only inputs
//!   that separate a `>> ss_x` from a `>> ss_y`, and both chroma ORIGINS are
//!   pinned by them (measured: swapping the two shifts fails).
//!   The CONV_BUF stride is NOT: all four C functions pass `is_compound = 0`
//!   and the single-prediction kernels never read `conv_params->dst`, so
//!   `>> ss_y` there leaves every cell green. That is stated in the port's
//!   module doc rather than claimed as covered.
//! * `sb_size` 64 and 128, since it IS the luma CONV_BUF stride.
//! * `pu_origin` / `dst_origin` values that are NOT multiples of 8, because
//!   `ROUND_UV(x)` is `((x) >> 3) << 3` (definitions.h:348) — it rounds down to
//!   a multiple of **8**, not to an even pair. Values below 8 all round to 0,
//!   which makes every shift of them inert; the first version of this test
//!   used `(3, 5)` / `(5, 7)` and a `>> ss_x`-instead-of-`>> ss_y` mutation on
//!   the destination origin passed. The origins here are large enough to
//!   distinguish the two shifts.

use svtav1_cref::interpred_gap::{
    EncMakePredArgs, RefDst, RefSrc, enc_make_inter_predictor as cref_emp,
};
use svtav1_dsp::port_convolve::InterpFilterKind;
use svtav1_dsp::port_enc_make_pred::{DstPlane, SrcPlanes};
use svtav1_dsp::port_inter_predictor::make_interp_filters;
use svtav1_dsp::port_obmc_build::round_uv;
use svtav1_dsp::port_obmc_single_pred::{
    ObmcPicDims, ObmcPlaneIo, get_single_prediction_for_obmc_chroma_plane,
    get_single_prediction_for_obmc_luma,
};
use svtav1_dsp::port_subpel_params::{MbEdges, Mv};

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

const BORDER: usize = 64;
const FRAME: i32 = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repr {
    Lbd,
    Split,
    Hbd,
}

struct Planes {
    msb: Vec<u8>,
    lsb: Vec<u8>,
    hbd: Vec<u16>,
    stride: usize,
}

impl Planes {
    fn new(seed: u32) -> Self {
        let stride = FRAME as usize + 2 * BORDER;
        let n = stride * stride;
        let mut s = seed | 1;
        let (mut msb, mut lsb, mut hbd) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..n {
            let v = xs(&mut s);
            let m = match v % 8 {
                0 => 0x00,
                1 => 0xFF,
                _ => (v >> 13) as u8,
            };
            let l = match (v >> 3) % 8 {
                0 => 0x00,
                1 => 0xC0,
                _ => (v >> 19) as u8,
            };
            msb.push(m);
            lsb.push(l);
            hbd.push((u16::from(m) << 2) | u16::from(l >> 6));
        }
        Self {
            msb,
            lsb,
            hbd,
            stride,
        }
    }
}

struct Cell {
    repr: Repr,
    sb_size: usize,
    plane: usize,
    ss: (i32, i32),
    mv: (i32, i32),
    pu: (u32, u32),
    dst_origin: (u32, u32),
    blk: (usize, usize),
}

fn run(c: &Cell, seed: u32) {
    let p = Planes::new(seed);
    let bd = if c.repr == Repr::Lbd { 8 } else { 10 };
    let is16bit = c.repr != Repr::Lbd;
    let filters = make_interp_filters(
        InterpFilterKind::EightTapRegular,
        InterpFilterKind::EightTapSmooth,
    );
    let (bw, bh) = c.blk;
    let dst_stride = 256usize;
    let dims = ObmcPicDims {
        reference: (FRAME, FRAME),
        prediction: (FRAME, FRAME),
        sb_size: c.sb_size,
    };
    let (pu_x, pu_y) = c.pu;
    let edges = MbEdges {
        to_left: -((pu_x as i32) * 8),
        to_right: (FRAME - pu_x as i32 - bw as i32) * 8,
        to_top: -((pu_y as i32) * 8),
        to_bottom: (FRAME - pu_y as i32 - bh as i32) * 8,
    };
    let conv_stride = if c.plane == 0 {
        c.sb_size
    } else {
        c.sb_size >> c.ss.0
    };
    let n_dst = dst_stride * 256;
    let mut r_dst_l = vec![0u8; n_dst];
    let mut r_dst_h = vec![0u16; n_dst];
    let mut c_dst_l = vec![0u8; n_dst];
    let mut c_dst_h = vec![0u16; n_dst];
    let mut r_cb = vec![0u16; conv_stride * 256];
    let mut c_cb = r_cb.clone();
    let r_seg = vec![0u8; 128 * 128];
    let mut c_seg = vec![0u8; 128 * 128];

    let reference = match c.repr {
        Repr::Lbd => SrcPlanes::Lbd(&p.msb),
        Repr::Split => SrcPlanes::Split {
            msb: &p.msb,
            lsb: &p.lsb,
        },
        Repr::Hbd => SrcPlanes::Hbd(&p.hbd),
    };
    let prediction = if is16bit {
        DstPlane::Hbd(&mut r_dst_h)
    } else {
        DstPlane::Lbd(&mut r_dst_l)
    };
    let io = ObmcPlaneIo {
        reference,
        src_stride: p.stride,
        prediction,
        dst_stride,
    };
    let mv = Mv {
        x: c.mv.0 as i16,
        y: c.mv.1 as i16,
    };
    if c.plane == 0 {
        get_single_prediction_for_obmc_luma(
            io,
            0,
            dims,
            filters,
            mv,
            pu_x,
            pu_y,
            c.dst_origin.0,
            c.dst_origin.1,
            bw,
            bh,
            &edges,
            &mut r_cb,
            bd,
        )
        .expect("regular leaf");
    } else {
        get_single_prediction_for_obmc_chroma_plane(
            io,
            0,
            dims,
            c.plane,
            filters,
            mv,
            pu_x,
            pu_y,
            c.dst_origin.0,
            c.dst_origin.1,
            bw,
            bh,
            &edges,
            c.ss.0,
            c.ss.1,
            &mut r_cb,
            bd,
        )
        .expect("regular leaf");
    }

    // C, driven with the parameter set the port just derived.
    let (mut cm, mut cl, mut ch) = (p.msb.clone(), p.lsb.clone(), p.hbd.clone());
    let csrc = match c.repr {
        Repr::Lbd => RefSrc::Lbd(&mut cm),
        Repr::Split => RefSrc::Split {
            msb: &mut cm,
            lsb: &mut cl,
        },
        Repr::Hbd => RefSrc::Hbd(&mut ch),
    };
    let (pre_x, pre_y, off) = if c.plane == 0 {
        (
            pu_x as i32,
            pu_y as i32,
            c.dst_origin.0 as usize + c.dst_origin.1 as usize * dst_stride,
        )
    } else {
        (
            (round_uv(pu_x) >> c.ss.0) as i32,
            (round_uv(pu_y) >> c.ss.1) as i32,
            (round_uv(c.dst_origin.0) >> c.ss.0) as usize
                + (round_uv(c.dst_origin.1) >> c.ss.1) as usize * dst_stride,
        )
    };
    let cdst = if is16bit {
        RefDst::Hbd(&mut c_dst_h[off..])
    } else {
        RefDst::Lbd(&mut c_dst_l[off..])
    };
    cref_emp(
        csrc,
        0,
        cdst,
        &mut c_cb,
        &mut c_seg,
        None,
        EncMakePredArgs {
            pre_y,
            pre_x,
            mv: c.mv,
            scale: (FRAME, FRAME, FRAME, FRAME),
            super_block_size: c.sb_size as i32,
            frame: (FRAME, FRAME),
            blk: (bw, bh),
            bsize: 3,
            edges: (edges.to_left, edges.to_right, edges.to_top, edges.to_bottom),
            interp_filters: filters,
            strides: (p.stride, dst_stride),
            conv_stride,
            compound: (false, false, false, 0, 0),
            plane: (c.plane, c.ss.1, c.ss.0),
            bit_depth: bd,
            use_intrabc: false,
            is16bit,
            masked: None,
        },
    );

    let what = format!(
        "{:?} plane{} sb{} ss{:?} mv{:?} pu{:?} dst{:?} {bw}x{bh}",
        c.repr, c.plane, c.sb_size, c.ss, c.mv, c.pu, c.dst_origin
    );
    assert_eq!(r_cb, c_cb, "CONV_BUF {what}");
    assert_eq!(r_seg, c_seg, "seg_mask {what}");
    if is16bit {
        assert_eq!(r_dst_h, c_dst_h, "dst {what}");
    } else {
        assert_eq!(r_dst_l, c_dst_l, "dst {what}");
    }
}

const MVS: [(i32, i32); 4] = [(0, 0), (7, -5), (33, 41), (-1024, 1024)];
const BLKS: [(usize, usize); 3] = [(8, 8), (16, 8), (32, 16)];

#[test]
fn obmc_luma_prediction_matches_c() {
    let mut cells = 0usize;
    for repr in [Repr::Lbd, Repr::Split, Repr::Hbd] {
        for sb_size in [64usize, 128] {
            for &blk in &BLKS {
                for (i, mv) in MVS.into_iter().enumerate() {
                    for pu in [(48u32, 48u32), (52, 53)] {
                        run(
                            &Cell {
                                repr,
                                sb_size,
                                plane: 0,
                                ss: (0, 0),
                                mv,
                                pu,
                                dst_origin: (24, 40),
                                blk,
                            },
                            0x5151_0001 ^ (i as u32) << 8 ^ (sb_size as u32) ^ blk.0 as u32,
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 144, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn obmc_chroma_prediction_matches_c() {
    let mut cells = 0usize;
    for repr in [Repr::Lbd, Repr::Split, Repr::Hbd] {
        for sb_size in [64usize, 128] {
            // (1, 1) is 4:2:0. (1, 0) and (0, 1) are the ASYMMETRIC pairs that
            // separate a `>> ss_x` CONV_BUF stride from a `>> ss_y` one, and
            // separate the two `ROUND_UV(..) >> ss_*` origins from each other.
            for ss in [(1i32, 1i32), (1, 0), (0, 1), (0, 0)] {
                for plane in [1usize, 2] {
                    for (i, mv) in MVS.into_iter().enumerate() {
                        for pu in [(48u32, 48u32), (52, 53)] {
                            run(
                                &Cell {
                                    repr,
                                    sb_size,
                                    plane,
                                    ss,
                                    mv,
                                    pu,
                                    dst_origin: (28, 45),
                                    blk: (8, 8),
                                },
                                0x6262_0001
                                    ^ (i as u32) << 8
                                    ^ (sb_size as u32)
                                    ^ (plane as u32) << 16
                                    ^ (ss.0 as u32) << 20,
                            );
                            cells += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(cells >= 384, "anti-vacuity: only {cells} cells ran");
}

/// POSITIVE CONTROL for the origins the sweep uses: `ROUND_UV(x)` is
/// `((x) >> 3) << 3` — a multiple of **8**, not an even pair — and the values
/// above must survive it NON-ZERO, or every shift applied to them is inert
/// and the cells that vary `ss` prove nothing.
///
/// This is the cell that caught it: with `dst_origin = (5, 7)` both components
/// round to 0, and a `>> ss_x`-instead-of-`>> ss_y` mutation on the
/// destination origin passed the whole suite.
#[test]
fn round_uv_origins_survive_non_zero_and_the_shifts_separate() {
    assert_eq!(round_uv(5), 0, "small values collapse — that was the trap");
    assert_eq!(round_uv(7), 0);
    // The sweep's origins.
    for (x, y) in [(24u32, 40u32), (28, 45)] {
        assert!(round_uv(x) > 0 && round_uv(y) > 0, "({x},{y}) collapses");
        assert_ne!(
            round_uv(x) >> 1,
            round_uv(x),
            "a shift of this value is not observable"
        );
        assert_ne!(
            round_uv(y) >> 1,
            round_uv(y),
            "a shift of this value is not observable"
        );
    }
    // And the two `pu` values must differ in LUMA while agreeing in CHROMA,
    // which is what makes the chroma cells exercise the rounding itself.
    assert_ne!((48u32, 48u32), (52u32, 53u32));
    assert_eq!((round_uv(48), round_uv(48)), (round_uv(52), round_uv(53)));
}
