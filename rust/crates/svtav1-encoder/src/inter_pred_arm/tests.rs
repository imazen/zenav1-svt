use super::*;
use crate::picture::PaddedPlaneT;
use svtav1_dsp::port_inter_predictor::broadcast_interp_filter;
use svtav1_dsp::port_warp::get_shear_params;
use svtav1_types::motion::{TransformationType, WarpedMotionParams};

fn xs(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// A non-identity ROTZOOM model (a small zoom about the block) with the
/// derived shear terms `av1_warp_plane` reads — `get_shear_params` fills
/// them exactly as the search does.
fn rotzoom_model() -> WarpedMotionParams {
    let mut wm = WarpedMotionParams {
        wm_type: TransformationType::RotZoom,
        wmmat: [0, 0, (1 << 16) + 512, 0, 0, 1 << 16],
        ..Default::default()
    };
    assert!(
        get_shear_params(&mut wm),
        "the test model must be warp-legal"
    );
    wm
}

struct CompoundCase {
    refs: [PaddedPlane; 2],
    refs_uv: [[PaddedPlane; 2]; 2],
    w: usize,
    h: usize,
}

impl CompoundCase {
    fn new(w: usize, h: usize) -> Self {
        let mut s = 0x9e37_79b9u32;
        let border = 64;
        let plane = |pw: usize, ph: usize, s: &mut u32| {
            let px: alloc::vec::Vec<u8> = (0..pw * ph).map(|_| (xs(s) >> 13) as u8).collect();
            PaddedPlaneT::from_plane(&px, pw, ph, border)
        };
        Self {
            refs: [plane(w, h, &mut s), plane(w, h, &mut s)],
            refs_uv: [
                [plane(w / 2, h / 2, &mut s), plane(w / 2, h / 2, &mut s)],
                [plane(w / 2, h / 2, &mut s), plane(w / 2, h / 2, &mut s)],
            ],
            w,
            h,
        }
    }
}

/// With `is_wm = [false, false]` the warp compound driver is C's
/// `av1_inter_prediction` compound loop restricted to the convolve leaf:
/// ref 0 fills the shared CONV_BUF (`do_average = 0`), ref 1 blends into
/// the destination (`do_average = 1`) — the same sequencing
/// `predict_inter_yuv_compound` (the `light_pd1` driver) runs. The
/// outputs must be bit-equal on every plane; a dropped shared buffer or
/// swapped `do_average` shows up here.
#[test]
fn warped_compound_convolve_fallback_matches_light_pd1() {
    let case = CompoundCase::new(64, 64);
    let (bw, bh) = (16usize, 16usize);
    let (cw, chh) = (bw / 2, bh / 2);
    let mvs = [Mv { x: 10, y: -6 }, Mv { x: -12, y: 4 }];
    let filters =
        broadcast_interp_filter(svtav1_dsp::port_convolve::InterpFilterKind::EightTapRegular);

    let (mut y_a, mut u_a, mut v_a) =
        (vec![0u8; bw * bh], vec![0u8; cw * chh], vec![0u8; cw * chh]);
    predict_inter_yuv_compound(
        [
            (&case.refs[0], &case.refs_uv[0][0], &case.refs_uv[0][1]),
            (&case.refs[1], &case.refs_uv[1][0], &case.refs_uv[1][1]),
        ],
        16,
        16,
        bw,
        bh,
        mvs,
        filters,
        128,
        case.w,
        case.h,
        &mut y_a,
        bw,
        &mut u_a,
        &mut v_a,
        cw,
    );

    let (mut y_b, mut u_b, mut v_b) =
        (vec![0u8; bw * bh], vec![0u8; cw * chh], vec![0u8; cw * chh]);
    let (mut wm0, mut wm1) = (WarpedMotionParams::default(), WarpedMotionParams::default());
    predict_inter_yuv_warped_compound(
        [
            (
                &case.refs[0],
                Some((&case.refs_uv[0][0], &case.refs_uv[0][1])),
            ),
            (
                &case.refs[1],
                Some((&case.refs_uv[1][0], &case.refs_uv[1][1])),
            ),
        ],
        &mut wm0,
        &mut wm1,
        [false, false],
        16,
        16,
        bw,
        bh,
        mvs,
        filters,
        128,
        case.w,
        case.h,
        &mut y_b,
        bw,
        &mut u_b,
        &mut v_b,
        cw,
    );

    assert_eq!(y_a, y_b, "luma convolve compound must match light_pd1");
    assert_eq!(u_a, u_b, "chroma convolve compound must match light_pd1");
    assert_eq!(v_a, v_b, "chroma convolve compound must match light_pd1");
}

/// The same candidate with ref 0's model above TRANSLATION must actually
/// warp — the whole point of the driver is that the decoder warps each
/// reference by its own model. If the `is_wm` flag were dropped the
/// prediction would equal the pure-convolve output, and the stream would
/// decode differently from the encoder's recon.
#[test]
fn warped_compound_engages_the_warp_leaf() {
    let case = CompoundCase::new(64, 64);
    let (bw, bh) = (16usize, 16usize);
    let mvs = [Mv { x: 10, y: -6 }, Mv { x: -12, y: 4 }];
    let filters =
        broadcast_interp_filter(svtav1_dsp::port_convolve::InterpFilterKind::EightTapRegular);

    let run = |is_wm: [bool; 2]| -> alloc::vec::Vec<u8> {
        let mut y = vec![0u8; bw * bh];
        let (mut u, mut v) = (vec![0u8; 8 * 8], vec![0u8; 8 * 8]);
        let mut wm0 = rotzoom_model();
        let mut wm1 = WarpedMotionParams::default();
        predict_inter_yuv_warped_compound(
            [
                (
                    &case.refs[0],
                    Some((&case.refs_uv[0][0], &case.refs_uv[0][1])),
                ),
                (
                    &case.refs[1],
                    Some((&case.refs_uv[1][0], &case.refs_uv[1][1])),
                ),
            ],
            &mut wm0,
            &mut wm1,
            is_wm,
            16,
            16,
            bw,
            bh,
            mvs,
            filters,
            128,
            case.w,
            case.h,
            &mut y,
            bw,
            &mut u,
            &mut v,
            8,
        );
        y
    };

    let convolve_only = run([false, false]);
    let warped_ref0 = run([true, false]);
    assert_ne!(
        convolve_only, warped_ref0,
        "a ROTZOOM model on ref 0 must change the compound prediction"
    );
}

/// Chroma is all-or-nothing: when neither reference carries it the luma
/// half must still land and the function must not touch the (empty)
/// chroma outputs.
#[test]
fn warped_compound_without_chroma_predicts_luma() {
    let case = CompoundCase::new(64, 64);
    let (bw, bh) = (16usize, 16usize);
    let mvs = [Mv { x: 8, y: 8 }, Mv { x: -8, y: -8 }];
    let filters =
        broadcast_interp_filter(svtav1_dsp::port_convolve::InterpFilterKind::EightTapRegular);
    let mut y = vec![0u8; bw * bh];
    let (mut wm0, mut wm1) = (rotzoom_model(), rotzoom_model());
    predict_inter_yuv_warped_compound(
        [(&case.refs[0], None), (&case.refs[1], None)],
        &mut wm0,
        &mut wm1,
        [true, true],
        16,
        16,
        bw,
        bh,
        mvs,
        filters,
        128,
        case.w,
        case.h,
        &mut y,
        bw,
        &mut [],
        &mut [],
        0,
    );
    assert!(y.iter().any(|&p| p != 0), "luma prediction must be written");
}
