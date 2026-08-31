//! Differential (evidence tier 1): `port_frame_update::frame_update_type` vs the
//! REAL exported `svt_aom_get_frame_update_type` (`Codec/resize.c:1246`), driven
//! through `svtav1-cref`'s `ref_get_frame_update_type` shim.
//!
//! The shim calloc's a `PictureParentControlSet` per call and sets exactly the
//! three fields the C function reads (`frm_hdr.frame_type`,
//! `hierarchical_levels`, `temporal_layer_index`), so this drives the C code
//! itself rather than a second transcription of it.

use svtav1_encoder::port_frame_update::{FrameUpdateType, frame_update_type};
use svtav1_types::frame::FrameType;

fn ft_from_i32(v: i32) -> FrameType {
    match v {
        0 => FrameType::Key,
        1 => FrameType::Inter,
        2 => FrameType::IntraOnly,
        3 => FrameType::Switch,
        _ => unreachable!(),
    }
}

/// Exhaustive over the reachable domain: every AV1 frame type x every
/// hierarchical level SVT can configure (0..=6, `MAX_HIERARCHICAL_LEVEL` is 6 —
/// `EbSvtAv1Enc.h`) x every temporal layer index up to 7.
#[test]
fn frame_update_type_matches_c_exhaustively() {
    let mut seen_kf = 0usize;
    let mut seen_lf = 0usize;
    let mut seen_arf = 0usize;
    let mut seen_intnl = 0usize;

    for ft in 0..4i32 {
        for hier in 0..=6i32 {
            for tl in 0..=7i32 {
                let c = svtav1_cref::get_frame_update_type(ft, hier, tl);
                let rust = frame_update_type(ft_from_i32(ft), hier as u8, tl as u8);
                assert_eq!(
                    rust.as_i32(),
                    c,
                    "frame_update_type mismatch: frame_type={ft} hier={hier} tl={tl} \
                     (rust {rust:?} = {}, C = {c})",
                    rust.as_i32()
                );
                match rust {
                    FrameUpdateType::Kf => seen_kf += 1,
                    FrameUpdateType::Lf => seen_lf += 1,
                    FrameUpdateType::Arf => seen_arf += 1,
                    FrameUpdateType::IntnlArf => seen_intnl += 1,
                    other => panic!("unreachable update type produced: {other:?}"),
                }
            }
        }
    }

    // Anti-vacuity: all four reachable arms were actually exercised. A probe
    // that only ever hits one arm proves nothing about the other three
    // (WORKING-ON-THIS.md section 5 — "before you trust a ZERO, prove the probe
    // fires").
    assert!(seen_kf > 0, "KF_UPDATE arm never reached");
    assert!(seen_lf > 0, "LF_UPDATE arm never reached");
    assert!(seen_arf > 0, "ARF_UPDATE arm never reached");
    assert!(seen_intnl > 0, "INTNL_ARF_UPDATE arm never reached");
}

/// The C enum discriminants are load-bearing — they index
/// `rd_frame_type_factor[]` — so pin the four values the classifier can return
/// against what C actually returns, not against the port's own enum.
#[test]
fn c_discriminants_are_the_documented_ones() {
    // KEY frame, any GOP shape -> SVT_AV1_KF_UPDATE = 0.
    assert_eq!(svtav1_cref::get_frame_update_type(0, 4, 0), 0);
    // Inter, hierarchical, base layer -> SVT_AV1_ARF_UPDATE = 3.
    assert_eq!(svtav1_cref::get_frame_update_type(1, 4, 0), 3);
    // Inter, hierarchical, top layer -> SVT_AV1_LF_UPDATE = 1.
    assert_eq!(svtav1_cref::get_frame_update_type(1, 4, 4), 1);
    // Inter, hierarchical, middle layer -> SVT_AV1_INTNL_ARF_UPDATE = 6.
    assert_eq!(svtav1_cref::get_frame_update_type(1, 4, 2), 6);
    // Inter, flat GOP -> SVT_AV1_LF_UPDATE = 1.
    assert_eq!(svtav1_cref::get_frame_update_type(1, 0, 0), 1);
}
