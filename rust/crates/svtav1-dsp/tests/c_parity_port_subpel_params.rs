//! Differential parity for the MV -> `SubpelParams` derivation — evidence
//! tier 1 (`WORKING-ON-THIS.md` §4) by composition.
//!
//! `compute_subpel_params` and `clamp_mv_to_umv_border_sb` are both `static`.
//! `tf_inter_predictor` (enc_inter_prediction.c:2452) IS exported and is the
//! only caller whose arguments a shim can synthesise: it reads exactly
//! `scs->super_block_size` off the `SequenceControlSet` and the four
//! `mb_to_*_edge` fields off the `MacroBlockD`. Everything the two static
//! functions compute lands in `tf_inter_predictor`'s output pixels — `pos_x` /
//! `pos_y` choose which source samples are read, `subpel_x` / `subpel_y` choose
//! the filter phase, and `xs` / `ys` choose which kernel the dispatcher runs —
//! so a difference in either function is a difference in the block.
//!
//! The port's side rebuilds `tf_inter_predictor` out of
//! `port_subpel_params::compute_subpel_params` plus
//! `port_inter_predictor::inter_predictor`, both of which are ported here, and
//! the two must produce the same pixels for every cell.

use svtav1_cref::inter_pred as cref;
use svtav1_dsp::port_convolve::{ConvolveParams, InterpFilterKind, SrcView};
use svtav1_dsp::port_inter_predictor::{
    broadcast_interp_filter, inter_predictor, make_interp_filters,
};
use svtav1_dsp::port_scale_factors::ScaleFactors;
use svtav1_dsp::port_subpel_params::{MbEdges, Mv, RefGeometry, compute_subpel_params};

const PAD: usize = 160;

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

fn plane(n: usize, seed: u32) -> Vec<u8> {
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

/// Both arms of `compute_subpel_params`: the unscaled one (which clamps the MV
/// against the UMV border) and the scaled one (which clamps the POSITION
/// against the padded reference extent).
#[test]
fn tf_inter_predictor_matches_c() {
    let mut cells = 0usize;
    let mut scaled_cells = 0usize;
    let mut unscaled_cells = 0usize;
    let mut clamped_cells = 0usize;

    for (blk_w, blk_h) in [(8usize, 8usize), (16, 16), (8, 16), (32, 32)] {
        for filters in [
            broadcast_interp_filter(InterpFilterKind::EightTapRegular),
            make_interp_filters(
                InterpFilterKind::EightTapSmooth,
                InterpFilterKind::MultiTapSharp,
            ),
            broadcast_interp_filter(InterpFilterKind::Bilinear),
        ] {
            // (other_w, other_h, this_w, this_h).
            //
            // MEASURED, and the reason for the last three: with only 1:1,
            // 2:1 and 96:128 the `+ SCALE_EXTRA_OFF` term is INVISIBLE.
            // Those ratios all give a y_scale_fp whose scaled position is a
            // multiple of 64, and SCALE_EXTRA_OFF is 32 — half a filter-phase
            // quantum — so it never crosses a phase boundary. Zeroing it in
            // the port did not fail this cell until these three odd ratios
            // (y_scale_fp 22528 / 13184 / 19200) were added; 30 of the 135
            // scaled cells now move a phase or a position when it is dropped.
            for sizes in [
                (128i32, 128i32, 128i32, 128i32),
                (256, 256, 128, 128),
                (96, 128, 128, 128),
                (130, 176, 128, 128),
                (97, 103, 128, 128),
                (200, 150, 128, 128),
            ] {
                for (mv_x, mv_y) in [
                    (0i32, 0i32),
                    (5, -3),
                    (17, 33),
                    (-40, -40),
                    // Deliberately far outside the frame, so the UMV clamp fires.
                    (4000, -4000),
                    (-4000, 4000),
                    (1, 1),
                    (3, 7),
                    (-9, 11),
                ] {
                    for (pre_x, pre_y) in [(16i32, 16i32), (0, 0), (40, 24)] {
                        let sf = ScaleFactors::setup_for_frame(sizes.0, sizes.1, sizes.2, sizes.3);
                        let edges = MbEdges {
                            to_left: -(pre_x * 8),
                            to_right: (128 - pre_x - blk_w as i32) * 8,
                            to_top: -(pre_y * 8),
                            to_bottom: (128 - pre_y - blk_h as i32) * 8,
                        };
                        let geom = RefGeometry {
                            super_block_size: 64,
                            frame_width: 128,
                            frame_height: 128,
                        };

                        let stride = 512usize;
                        let rows = 512usize;
                        let src = plane(
                            stride * rows,
                            0x51B ^ blk_w as u32 ^ mv_x as u32 ^ sizes.0 as u32,
                        );
                        let origin = PAD * stride + PAD;

                        // --- port side: derive, then predict.
                        let (sp, pos_y, pos_x) = compute_subpel_params(
                            geom,
                            pre_y,
                            pre_x,
                            Mv {
                                x: mv_x as i16,
                                y: mv_y as i16,
                            },
                            &sf,
                            blk_w as i32,
                            blk_h as i32,
                            &edges,
                            0,
                            0,
                        );
                        let mut r_dst = vec![0u8; blk_w * blk_h];
                        let mut cb = vec![0u16; blk_w * blk_h];
                        let cp = ConvolveParams::single(false, 8);
                        let shifted =
                            (origin as isize + pos_x as isize + pos_y as isize * stride as isize)
                                as usize;
                        inter_predictor(
                            SrcView::new(&src, shifted, stride),
                            &mut r_dst,
                            blk_w,
                            &mut cb,
                            &sp,
                            blk_w,
                            blk_h,
                            &cp,
                            filters,
                            false,
                        );

                        // --- C side: the whole thing.
                        let mut c_src = src.clone();
                        let mut c_dst = vec![0u8; blk_w * blk_h];
                        cref::tf_inter_predictor(
                            &mut c_src,
                            origin,
                            stride,
                            &mut c_dst,
                            blk_w,
                            (pre_y, pre_x),
                            (mv_x, mv_y),
                            sizes,
                            64,
                            (128, 128),
                            (blk_w as i32, blk_h as i32),
                            cref::RefMbEdges {
                                to_left: edges.to_left,
                                to_right: edges.to_right,
                                to_top: edges.to_top,
                                to_bottom: edges.to_bottom,
                            },
                            filters,
                            8,
                            0,
                        );

                        assert_eq!(
                            r_dst, c_dst,
                            "tf_inter_predictor {blk_w}x{blk_h} mv({mv_x},{mv_y}) pre({pre_x},{pre_y}) sizes {sizes:?}"
                        );

                        if sf.is_scaled() {
                            scaled_cells += 1;
                        } else {
                            unscaled_cells += 1;
                            // Did the UMV clamp actually bind?
                            let doubled = (mv_x * 2) as i16 as i32;
                            if doubled != (sp.subpel_x >> 6) + ((pos_x - pre_x) << 4) {
                                clamped_cells += 1;
                            }
                        }
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells >= 600, "anti-vacuity: only {cells} cells ran");
    assert!(
        scaled_cells > 100,
        "the scaled arm ran only {scaled_cells} times"
    );
    assert!(
        unscaled_cells > 100,
        "the unscaled arm ran only {unscaled_cells} times"
    );
    assert!(
        clamped_cells > 20,
        "the UMV clamp bound only {clamped_cells} times"
    );
}
