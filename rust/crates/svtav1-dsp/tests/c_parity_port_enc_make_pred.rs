//! Differential parity for `svt_aom_enc_make_inter_predictor` — evidence
//! tier 1 (`WORKING-ON-THIS.md` §4).
//!
//! Symbol driven: `svt_aom_enc_make_inter_predictor`
//! (enc_inter_prediction.c:2515), `nm -g`-visible as `T`. The `static`
//! `av1_make_masked_scaled_inter_predictor` (:77) is driven indirectly and
//! completely — the masked leaf's whole body is that call.
//!
//! # Coverage, as a fraction
//!
//! **2 of C's 4 leaves.** The regular leaf and the masked-compound leaf run
//! here across all three source representations
//! (`Lbd` / `Split` / `Hbd`), both compound mask types (DIFFWTD and WEDGE),
//! and both scaled and unscaled references. The two WARP leaves are NOT
//! driven: the port returns `MakePredError::WarpNotWired` for them, and
//! [`warp_is_refused_not_approximated`] pins that so the refusal cannot rot
//! into a silent wrong prediction.
//!
//! # The contract this hands C (§5 trap 4)
//!
//! * The reference planes carry a 64-sample border on every side, because the
//!   MV clamp only bounds the position to `AOM_INTERP_EXTEND` PAST the block
//!   and the kernels then read 3 back / 4 forward from there.
//! * `src_origin` is passed UNADJUSTED: C applies its own
//!   `pos_x + pos_y * src_stride` offset inside the function, and pre-applying
//!   it would double-count.
//! * `seg_mask` is `BLOCK_W[bsize] * BLOCK_H[bsize]` bytes, which is the
//!   stride the BLEND reads it at — writing it at `w` and reading it at
//!   `BLOCK_W[bsize]` only agrees because every cell picks a `bsize` whose
//!   width IS `w`.

use svtav1_cref::interpred_gap::{
    EncMakePredArgs, RefDst, RefSrc, enc_make_inter_predictor as cref_emp,
};
use svtav1_dsp::port_convolve::{ConvolveParams, InterpFilterKind};
use svtav1_dsp::port_enc_make_pred::{
    DstPlane, MakePredError, MaskedCompound, SrcPlanes, enc_make_inter_predictor,
};
use svtav1_dsp::port_inter_predictor::make_interp_filters;
use svtav1_dsp::port_masked_blend::InterInterCompoundData;
use svtav1_dsp::port_masked_compound::{CompoundType, DiffwtdMaskType};
use svtav1_dsp::port_scale_factors::ScaleFactors;
use svtav1_dsp::port_subpel_params::{MbEdges, Mv, RefGeometry};
use svtav1_dsp::port_wedge_masks::WedgeMasks;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// Border on every side of the reference plane, in samples.
const BORDER: usize = 64;

struct Planes {
    msb: Vec<u8>,
    lsb: Vec<u8>,
    hbd: Vec<u16>,
    stride: usize,
    origin: usize,
}

impl Planes {
    fn new(w: usize, h: usize, seed: u32) -> Self {
        let stride = w + 2 * BORDER;
        let rows = h + 2 * BORDER;
        let mut s = seed | 1;
        let n = stride * rows;
        let mut msb = Vec::with_capacity(n);
        let mut lsb = Vec::with_capacity(n);
        let mut hbd = Vec::with_capacity(n);
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
                2 => 0x3F,
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
            origin: 0,
        }
    }
}

/// Which representation a cell uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repr {
    Lbd,
    Split,
    Hbd,
}

const BLOCK_W: [usize; 22] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, 4, 16, 8, 32, 16, 64,
];
const BLOCK_H: [usize; 22] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, 16, 4, 32, 8, 64, 16,
];

/// `(bsize, w, h)` — square and rectangular, all with `BLOCK_W[bsize] == w`.
const CELLS: [(usize, usize, usize); 5] =
    [(3, 8, 8), (6, 16, 16), (5, 16, 8), (4, 8, 16), (9, 32, 32)];

struct Cell {
    bsize: usize,
    w: usize,
    h: usize,
    repr: Repr,
    mv: (i32, i32),
    filters: u32,
    scale: (i32, i32, i32, i32),
    masked: Option<(CompoundType, DiffwtdMaskType)>,
    is_compound: bool,
    /// `conv_params->do_average`. On the MASKED leaf C clears it (:2593), so a
    /// cell with `do_average = true` AND a mask is the only thing that can see
    /// that clear; every other cell is inert to it.
    do_average: bool,
}

fn run(cell: &Cell, seed: u32) {
    let (w, h, bsize) = (cell.w, cell.h, cell.bsize);
    let bd = if cell.repr == Repr::Lbd { 8 } else { 10 };
    let is16bit = cell.repr != Repr::Lbd;
    let p = Planes::new(256, 256, seed);
    // The block sits well inside the plane so the clamped position plus the
    // filter taps stay in bounds for every MV below.
    let (pre_x, pre_y) = (96i32, 96i32);
    let edges = MbEdges {
        to_left: -(pre_x * 8),
        to_right: (256 - pre_x - w as i32) * 8,
        to_top: -(pre_y * 8),
        to_bottom: (256 - pre_y - h as i32) * 8,
    };
    let conv_stride = w;
    let mut cp = ConvolveParams::no_round(cell.do_average, conv_stride, cell.is_compound, bd);
    cp.do_average = cell.do_average;
    let sf = ScaleFactors::setup_for_frame(cell.scale.0, cell.scale.1, cell.scale.2, cell.scale.3);
    let geom = RefGeometry {
        super_block_size: 64,
        frame_width: 256,
        frame_height: 256,
    };
    let mv = Mv {
        x: cell.mv.0 as i16,
        y: cell.mv.1 as i16,
    };
    let wedge = WedgeMasks::new();
    let seg_len = BLOCK_W[bsize] * BLOCK_H[bsize];

    // Reference 0's CONV_BUF: both sides get the same bytes, standing in for
    // the regular pass the encoder ran before the masked one.
    let mut s = seed ^ 0x5150_1234;
    let ref0: Vec<u16> = (0..conv_stride * h)
        .map(|_| (xs(&mut s) % 20000 + 2000) as u16)
        .collect();

    let (mut r_cb, mut c_cb) = (ref0.clone(), ref0.clone());
    let mut r_dst_l = vec![0u8; w * h];
    let mut r_dst_h = vec![0u16; w * h];
    let mut c_dst_l = vec![0u8; w * h];
    let mut c_dst_h = vec![0u16; w * h];
    let mut r_seg = vec![0u8; seg_len];
    let mut c_seg = vec![0u8; seg_len];

    let comp_data = cell.masked.map(|(t, _)| InterInterCompoundData {
        compound_type: t,
        wedge_index: 3,
        wedge_sign: 1,
    });

    // ---- port ----
    let src = match cell.repr {
        Repr::Lbd => SrcPlanes::Lbd(&p.msb),
        Repr::Split => SrcPlanes::Split {
            msb: &p.msb,
            lsb: &p.lsb,
        },
        Repr::Hbd => SrcPlanes::Hbd(&p.hbd),
    };
    let dst = if is16bit {
        DstPlane::Hbd(&mut r_dst_h)
    } else {
        DstPlane::Lbd(&mut r_dst_l)
    };
    let masked = comp_data.as_ref().map(|c| MaskedCompound {
        comp: c,
        seg_mask: &mut r_seg,
        wedge: &wedge,
        bsize,
        mask_type: cell.masked.unwrap().1,
    });
    enc_make_inter_predictor(
        src,
        p.origin,
        p.stride,
        dst,
        w,
        &mut r_cb,
        pre_y,
        pre_x,
        mv,
        &sf,
        &cp,
        cell.filters,
        masked,
        geom,
        w,
        h,
        &edges,
        0,
        0,
        0,
        bd,
        false,
        false,
    )
    .expect("non-warp leaf");

    // ---- C ----
    let (mut c_msb, mut c_lsb, mut c_hbd) = (p.msb.clone(), p.lsb.clone(), p.hbd.clone());
    let csrc = match cell.repr {
        Repr::Lbd => RefSrc::Lbd(&mut c_msb),
        Repr::Split => RefSrc::Split {
            msb: &mut c_msb,
            lsb: &mut c_lsb,
        },
        Repr::Hbd => RefSrc::Hbd(&mut c_hbd),
    };
    let cdst = if is16bit {
        RefDst::Hbd(&mut c_dst_h)
    } else {
        RefDst::Lbd(&mut c_dst_l)
    };
    cref_emp(
        csrc,
        p.origin,
        cdst,
        &mut c_cb,
        &mut c_seg,
        EncMakePredArgs {
            pre_y,
            pre_x,
            mv: (cell.mv.0, cell.mv.1),
            scale: cell.scale,
            super_block_size: 64,
            frame: (256, 256),
            blk: (w, h),
            bsize: bsize as i32,
            edges: (edges.to_left, edges.to_right, edges.to_top, edges.to_bottom),
            interp_filters: cell.filters,
            strides: (p.stride, w),
            conv_stride,
            compound: (cell.is_compound, cell.do_average, false, 0, 0),
            plane: (0, 0, 0),
            bit_depth: bd,
            use_intrabc: false,
            is16bit,
            masked: cell.masked.map(|(t, mt)| (t as i32, 3, 1, mt as i32)),
        },
    );

    let what = format!(
        "{:?} {w}x{h} bsize{bsize} mv{:?} masked{:?} comp{} avg{}",
        cell.repr, cell.mv, cell.masked, cell.is_compound, cell.do_average
    );
    assert_eq!(r_cb, c_cb, "CONV_BUF {what}");
    assert_eq!(r_seg, c_seg, "seg_mask {what}");
    if is16bit {
        assert_eq!(r_dst_h, c_dst_h, "dst {what}");
    } else {
        assert_eq!(r_dst_l, c_dst_l, "dst {what}");
    }
}

const MVS: [(i32, i32); 5] = [(0, 0), (5, -3), (-17, 11), (129, -260), (-1024, 1024)];

#[test]
fn regular_leaf_matches_c() {
    let mut cells = 0usize;
    for repr in [Repr::Lbd, Repr::Split, Repr::Hbd] {
        for &(bsize, w, h) in &CELLS {
            for (i, mv) in MVS.into_iter().enumerate() {
                for (is_compound, do_average) in [(false, false), (true, false), (true, true)] {
                    let filters = make_interp_filters(
                        InterpFilterKind::EightTapSmooth,
                        InterpFilterKind::EightTapRegular,
                    );
                    run(
                        &Cell {
                            bsize,
                            w,
                            h,
                            repr,
                            mv,
                            filters,
                            scale: (256, 256, 256, 256),
                            masked: None,
                            is_compound,
                            do_average,
                        },
                        0x3131_0001 ^ (w as u32) << 8 ^ (i as u32) << 16 ^ bsize as u32,
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 200, "anti-vacuity: only {cells} cells ran");
}

#[test]
fn masked_compound_leaf_matches_c() {
    let mut cells = 0usize;
    for repr in [Repr::Lbd, Repr::Split, Repr::Hbd] {
        for &(bsize, w, h) in &CELLS {
            for (i, mv) in MVS.into_iter().enumerate() {
                for (masked, do_average) in [
                    ((CompoundType::DiffWtd, DiffwtdMaskType::D38), false),
                    ((CompoundType::DiffWtd, DiffwtdMaskType::D38), true),
                    ((CompoundType::DiffWtd, DiffwtdMaskType::D38Inv), false),
                    ((CompoundType::Wedge, DiffwtdMaskType::D38), false),
                    ((CompoundType::Wedge, DiffwtdMaskType::D38), true),
                ] {
                    let filters = make_interp_filters(
                        InterpFilterKind::MultiTapSharp,
                        InterpFilterKind::EightTapRegular,
                    );
                    run(
                        &Cell {
                            bsize,
                            w,
                            h,
                            repr,
                            mv,
                            filters,
                            scale: (256, 256, 256, 256),
                            masked: Some(masked),
                            is_compound: true,
                            do_average,
                        },
                        0x7272_0001 ^ (h as u32) << 8 ^ (i as u32) << 16 ^ bsize as u32,
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 300, "anti-vacuity: only {cells} cells ran");
}

/// The SCALED reference arm — a different `compute_subpel_params` branch, a
/// different kernel (`*_convolve_2d_scale`), and for `Split` a packed scratch
/// that is twice as wide per scaled axis.
#[test]
fn scaled_reference_matches_c() {
    let mut cells = 0usize;
    for repr in [Repr::Lbd, Repr::Split, Repr::Hbd] {
        for scale in [
            (512, 512, 256, 256),
            (512, 256, 256, 256),
            (256, 512, 256, 256),
        ] {
            for &(bsize, w, h) in &CELLS[..3] {
                for (i, mv) in MVS.into_iter().take(3).enumerate() {
                    let filters = make_interp_filters(
                        InterpFilterKind::EightTapRegular,
                        InterpFilterKind::EightTapSmooth,
                    );
                    run(
                        &Cell {
                            bsize,
                            w,
                            h,
                            repr,
                            mv,
                            filters,
                            scale,
                            masked: None,
                            is_compound: false,
                            do_average: false,
                        },
                        0x9393_0001 ^ (w as u32) << 8 ^ (i as u32) << 16 ^ scale.0 as u32,
                    );
                    cells += 1;
                }
            }
        }
    }
    assert!(cells >= 27, "anti-vacuity: only {cells} cells ran");
}

/// The two WARP leaves are REFUSED, not approximated (`WORKING-ON-THIS.md`
/// §6). This pins the refusal so it cannot become a silent wrong prediction,
/// and names the leaf so the gap is legible in a failure message.
#[test]
fn warp_is_refused_not_approximated() {
    use svtav1_dsp::port_make_pred::MakePredLeaf;
    let p = Planes::new(64, 64, 0x1234_0001);
    let mut dst = vec![0u8; 64];
    let mut cb = vec![0u16; 64];
    let wedge = WedgeMasks::new();
    let mut seg = vec![0u8; 64];
    let comp = InterInterCompoundData {
        compound_type: CompoundType::Wedge,
        wedge_index: 0,
        wedge_sign: 0,
    };
    for (masked, want) in [
        (false, MakePredLeaf::Warp),
        (true, MakePredLeaf::MaskedWarp),
    ] {
        let m = masked.then(|| MaskedCompound {
            comp: &comp,
            seg_mask: &mut seg,
            wedge: &wedge,
            bsize: 3,
            mask_type: DiffwtdMaskType::D38,
        });
        let err = enc_make_inter_predictor(
            SrcPlanes::Lbd(&p.msb),
            p.origin,
            p.stride,
            DstPlane::Lbd(&mut dst),
            8,
            &mut cb,
            8,
            8,
            Mv { x: 0, y: 0 },
            &ScaleFactors::setup_for_frame(64, 64, 64, 64),
            &ConvolveParams::no_round(false, 8, false, 8),
            0,
            m,
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
            0,
            0,
            0,
            8,
            false,
            true,
        )
        .unwrap_err();
        assert_eq!(err, MakePredError::WarpNotWired(want));
    }
    assert_eq!(dst, vec![0u8; 64], "a refused call wrote pixels");
}
