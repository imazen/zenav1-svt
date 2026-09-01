//! Differential parity for one OBMC neighbour's prediction — tier 1 for what
//! the walk contains, tier 4 for the walk (`WORKING-ON-THIS.md` §4).
//!
//! `build_prediction_by_above_pred` (:1120) and `build_prediction_by_left_pred`
//! (:1228) are `static`, so there is no symbol to bind. Their whole body is a
//! per-plane geometry derivation followed by one
//! `get_single_prediction_for_obmc_*` call per plane, and each of THOSE is one
//! `svt_aom_enc_make_inter_predictor` call — which is exported. So this test
//! drives C once per plane with the geometry the port derived, and compares
//! the assembled scratch. A wrong extent, corner, source position or plane
//! order fails here.
//!
//! # The asymmetry the sweep has to reach
//!
//! ABOVE halves the HEIGHT and walks its destination corner along X; LEFT
//! halves the WIDTH and walks it along Y; and they pass a different `dir` to
//! `svt_av1_skip_u4x4_pred_in_obmc`, so ABOVE can skip a chroma plane that
//! LEFT never skips. [`the_two_sides_are_not_mirror_images`] asserts the sweep
//! actually reaches a block size where the skip fires on one side and not the
//! other, instead of leaving that to inspection.

use svtav1_cref::interpred_gap::{
    EncMakePredArgs, RefDst, RefSrc, enc_make_inter_predictor as cref_emp,
};
use svtav1_dsp::port_convolve::InterpFilterKind;
use svtav1_dsp::port_enc_make_pred::{DstPlane, SrcPlanes};
use svtav1_dsp::port_inter_predictor::make_interp_filters;
use svtav1_dsp::port_obmc_build::round_uv;
use svtav1_dsp::port_obmc_nb_pred::{
    NbSide, Neighbour, ObmcRefPic, ObmcScratch, build_prediction_by_nb_pred, nb_pred_geometry,
};
use svtav1_dsp::port_subpel_params::{MbEdges, Mv};
use svtav1_types::block::BlockSize;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

const FRAME: i32 = 192;
const BORDER: usize = 64;
const SB_SIZE: usize = 64;
/// `PICTURE_BUFFER_DESC_FULL_MASK` — luma plus chroma.
const FULL_MASK: u32 = 7;

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

const BSIZES: [(BlockSize, usize, usize); 4] = [
    (BlockSize::Block8x8, 2, 2),
    (BlockSize::Block16x16, 4, 4),
    (BlockSize::Block32x16, 8, 4),
    (BlockSize::Block16x32, 4, 8),
];

fn run(
    side: NbSide,
    bsize: BlockSize,
    n4_w: usize,
    n4_h: usize,
    nb: Neighbour,
    seed: u32,
) -> usize {
    let stride = FRAME as usize + 2 * BORDER;
    // THREE DIFFERENT reference planes. With one shared plane a U/V swap in
    // the walk is invisible (both predictions read the same samples and write
    // to different buffers), which a mutation showed: swapping the chroma pair
    // left every cell green.
    let src = plane8(stride * stride, seed);
    let src_u = plane8(stride * stride, seed ^ 0x0f0f_0f0f);
    let src_v = plane8(stride * stride, seed ^ 0xf0f0_f0f0);
    let (mi_row, mi_col) = (12i32, 12i32);
    let (ss_x, ss_y) = (1i32, 1i32);
    let dst_stride = [128usize, 64, 64];
    let n = [dst_stride[0] * 128, dst_stride[1] * 64, dst_stride[2] * 64];
    let edges = MbEdges {
        to_left: -((mi_col * 4) * 8),
        to_right: (FRAME - mi_col * 4 - (n4_w as i32) * 4) * 8,
        to_top: -((mi_row * 4) * 8),
        to_bottom: (FRAME - mi_row * 4 - (n4_h as i32) * 4) * 8,
    };
    let filters = nb.interp_filters;

    let mut ry = vec![0u8; n[0]];
    let mut ru = vec![0u8; n[1]];
    let mut rv = vec![0u8; n[2]];
    let mut conv = vec![0u16; SB_SIZE * 256];

    build_prediction_by_nb_pred(
        side,
        ObmcRefPic {
            y: SrcPlanes::Lbd(&src),
            u: SrcPlanes::Lbd(&src_u),
            v: SrcPlanes::Lbd(&src_v),
            stride: [stride, stride, stride],
            dims: (FRAME, FRAME),
        },
        [0, 0, 0],
        ObmcScratch {
            y: DstPlane::Lbd(&mut ry),
            u: DstPlane::Lbd(&mut ru),
            v: DstPlane::Lbd(&mut rv),
            stride: dst_stride,
        },
        (FRAME, FRAME),
        SB_SIZE,
        bsize,
        mi_row,
        mi_col,
        nb,
        &edges,
        ss_x,
        ss_y,
        FULL_MASK,
        &mut conv,
        8,
    )
    .expect("regular leaf");

    // C, one call per plane, over the geometry the port derived.
    let geoms = nb_pred_geometry(
        side,
        bsize,
        mi_row,
        mi_col,
        nb,
        ss_x as usize,
        ss_y as usize,
        FULL_MASK,
    );
    let mut cy = vec![0u8; n[0]];
    let mut cu = vec![0u8; n[1]];
    let mut cv = vec![0u8; n[2]];
    let mut c_conv = vec![0u16; SB_SIZE * 256];
    let mut c_seg = vec![0u8; 128 * 128];
    let mut calls = 0usize;
    for g in &geoms {
        let planes: &[usize] = if g.plane == 0 { &[0] } else { &[1, 2] };
        for &plane in planes {
            let (pre_x, pre_y, off, conv_stride) = if plane == 0 {
                (
                    g.mi_x,
                    g.mi_y,
                    g.dst_origin_x + g.dst_origin_y * dst_stride[0],
                    SB_SIZE,
                )
            } else {
                (
                    (round_uv(g.mi_x as u32) >> ss_x) as i32,
                    (round_uv(g.mi_y as u32) >> ss_y) as i32,
                    (round_uv(g.dst_origin_x as u32) >> ss_x) as usize
                        + (round_uv(g.dst_origin_y as u32) >> ss_y) as usize * dst_stride[plane],
                    SB_SIZE >> ss_x,
                )
            };
            let mut c_src = match plane {
                0 => src.clone(),
                1 => src_u.clone(),
                _ => src_v.clone(),
            };
            let dst: &mut [u8] = match plane {
                0 => &mut cy[off..],
                1 => &mut cu[off..],
                _ => &mut cv[off..],
            };
            cref_emp(
                RefSrc::Lbd(&mut c_src),
                0,
                RefDst::Lbd(dst),
                &mut c_conv,
                &mut c_seg,
                None,
                EncMakePredArgs {
                    pre_y,
                    pre_x,
                    mv: (nb.mv.x as i32, nb.mv.y as i32),
                    scale: (FRAME, FRAME, FRAME, FRAME),
                    super_block_size: SB_SIZE as i32,
                    frame: (FRAME, FRAME),
                    blk: (g.bw, g.bh),
                    bsize: bsize as i32,
                    edges: (edges.to_left, edges.to_right, edges.to_top, edges.to_bottom),
                    interp_filters: filters,
                    strides: (stride, dst_stride[plane]),
                    conv_stride,
                    compound: (false, false, false, 0, 0),
                    plane: (
                        plane,
                        if plane == 0 { 0 } else { ss_y },
                        if plane == 0 { 0 } else { ss_x },
                    ),
                    bit_depth: 8,
                    use_intrabc: false,
                    is16bit: false,
                    masked: None,
                },
            );
            calls += 1;
        }
    }
    let what = format!("{side:?} {bsize:?} nb{nb:?}");
    assert_eq!(ry, cy, "Y {what}");
    assert_eq!(ru, cu, "U {what}");
    assert_eq!(rv, cv, "V {what}");
    calls
}

#[test]
fn nb_prediction_matches_c() {
    let mut cells = 0usize;
    let mut calls = 0usize;
    for side in [NbSide::Above, NbSide::Left] {
        for &(bsize, n4_w, n4_h) in &BSIZES {
            for (i, (mv_x, mv_y)) in [(0i32, 0i32), (11, -6), (-512, 512)]
                .into_iter()
                .enumerate()
            {
                for rel_mi in [0usize, 2] {
                    for extent_mi in [1usize, 2, 4] {
                        let nb = Neighbour {
                            mv: Mv {
                                x: mv_x as i16,
                                y: mv_y as i16,
                            },
                            interp_filters: make_interp_filters(
                                InterpFilterKind::EightTapRegular,
                                InterpFilterKind::EightTapSmooth,
                            ),
                            extent_mi,
                            rel_mi,
                        };
                        calls += run(
                            side,
                            bsize,
                            n4_w,
                            n4_h,
                            nb,
                            0x7a7a_0001 ^ (i as u32) << 8 ^ (rel_mi as u32) << 12,
                        );
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 144, "anti-vacuity: only {cells} cells ran");
    assert!(
        calls >= cells,
        "every cell must have made at least one C call; {calls} for {cells} cells"
    );
}

/// POSITIVE CONTROL: the two walks are not mirror images. The sweep must reach
/// a case where the ABOVE walk's `svt_av1_skip_u4x4_pred_in_obmc(bsize, 0, ..)`
/// skips a plane that the LEFT walk's `dir = 1` does NOT — otherwise every
/// cell above would be blind to the `dir` argument.
#[test]
fn the_two_sides_are_not_mirror_images() {
    let nb = Neighbour {
        mv: Mv { x: 0, y: 0 },
        interp_filters: 0,
        extent_mi: 1,
        rel_mi: 0,
    };
    let mut plane_sets_differ = 0usize;
    let mut shapes_are_not_transposes = 0usize;
    for &(bsize, _, _) in &BSIZES {
        let a = nb_pred_geometry(NbSide::Above, bsize, 12, 12, nb, 1, 1, FULL_MASK);
        let l = nb_pred_geometry(NbSide::Left, bsize, 12, 12, nb, 1, 1, FULL_MASK);
        let a_planes: Vec<usize> = a.iter().map(|g| g.plane).collect();
        let l_planes: Vec<usize> = l.iter().map(|g| g.plane).collect();
        if a_planes != l_planes {
            // Only `svt_av1_skip_u4x4_pred_in_obmc`'s `dir` can do this: it
            // returns `dir == 0` when the CHROMA plane block is 4x4 / 8x4 /
            // 4x8, so ABOVE skips a plane LEFT keeps.
            plane_sets_differ += 1;
        }
        // A SQUARE block's two geometries legitimately ARE transposes (each
        // halves one dimension of the same square and takes the neighbour's
        // extent for the other). A NON-square one is not, because the halved
        // dimension comes from `bsize` and the other from the neighbour.
        if let (Some(ga), Some(gl)) = (a.first(), l.first()) {
            if (ga.bw, ga.bh) != (gl.bh, gl.bw) {
                shapes_are_not_transposes += 1;
            }
        }
    }
    assert!(
        plane_sets_differ > 0,
        "no block size in the sweep reaches a plane set where the two `dir` \
         values disagree — the `dir` argument is untested"
    );
    assert!(
        shapes_are_not_transposes > 0,
        "every block size in the sweep gives transposed geometries, so the \
         sweep cannot tell the two walks apart on shape alone"
    );
}
