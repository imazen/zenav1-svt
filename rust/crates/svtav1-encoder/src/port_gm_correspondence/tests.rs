use super::*;
use alloc::vec;

/// One b64, `pu_count` slots, one candidate each, all list-0 ref-0, with
/// `me_mv_array[n_idx]` = a distinguishable MV.
fn one_b64(pu_count: usize) -> (Vec<u8>, Vec<MeCandidateRef>, Vec<Mv>) {
    let totals = vec![1u8; pu_count];
    let cands = vec![
        MeCandidateRef {
            direction: 0,
            ref_idx_l0: 0,
            ref_idx_l1: 0,
            ref0_list: 0,
            ref1_list: 0,
        };
        pu_count
    ];
    let mvs: Vec<Mv> = (0..pu_count)
        .map(|n| Mv {
            x: n as i16,
            y: -(n as i16),
        })
        .collect();
    (totals, cands, mvs)
}

fn view<'a>(
    totals: &'a [u8],
    cands: &'a [MeCandidateRef],
    mvs: &'a [Mv],
    pu_count: usize,
) -> MeResultsView<'a> {
    MeResultsView {
        total_me_candidate_index: totals,
        me_candidate_array: cands,
        me_mv_array: mvs,
        pu_count,
        max_cand: 1,
        max_refs: 1,
        max_l0: 1,
    }
}

fn geom(w: u32, h: u32) -> GmPictureGeometry {
    GmPictureGeometry {
        aligned_width: w,
        aligned_height: h,
        b64_size: 64,
        enable_me_8x8: true,
        enable_me_16x16: true,
        gm_downsample_level: GmDownsampleLevel::Full,
    }
}

#[test]
fn mv64x64_emits_one_correspondence_per_b64() {
    let (t, c, m) = one_b64(85);
    let v = view(&t, &c, &m, 85);
    let out = correspondence_from_mvs(&v, &geom(64, 64), CorrespondenceMethod::Mv64x64, 0, 0);
    assert_eq!(out.len(), 1);
    // starting_n_idx 0 -> me_mv_array[0] = (0, 0).
    assert_eq!(
        out[0],
        Correspondence {
            x: 0,
            y: 0,
            rx: 0,
            ry: 0
        }
    );
}

#[test]
fn mv32x32_walks_four_blocks_in_raster_order_with_starting_index_one() {
    let (t, c, m) = one_b64(85);
    let v = view(&t, &c, &m, 85);
    let out = correspondence_from_mvs(&v, &geom(64, 64), CorrespondenceMethod::Mv32x32, 0, 0);
    assert_eq!(out.len(), 4);
    // i = 0..4 -> n_idx 1..5 -> mv (n, -n); position (i%2*32, i/2*32).
    for (i, corr) in out.iter().enumerate() {
        let bx = ((i % 2) * 32) as i32;
        let by = ((i / 2) * 32) as i32;
        let n = (1 + i) as i32;
        assert_eq!(
            *corr,
            Correspondence {
                x: bx,
                y: by,
                rx: bx + n,
                ry: by - n
            }
        );
    }
}

/// Blocks whose TOP-LEFT is outside the aligned frame are skipped; one
/// that starts inside is kept even though it extends past the edge.
#[test]
fn out_of_frame_blocks_are_skipped_by_top_left_only() {
    let (t, c, m) = one_b64(85);
    let v = view(&t, &c, &m, 85);
    // 40x40 aligned: only the block at (0,0) starts inside; (32,0),
    // (0,32) and (32,32) all start at >= 40? No — 32 < 40, so three of
    // the four ALSO start inside and are kept despite overhanging.
    let out = correspondence_from_mvs(&v, &geom(40, 40), CorrespondenceMethod::Mv32x32, 0, 0);
    assert_eq!(
        out.len(),
        4,
        "a block starting inside is kept even if it overhangs"
    );
    // 24x24: only (0,0) starts inside.
    let out = correspondence_from_mvs(&v, &geom(24, 24), CorrespondenceMethod::Mv32x32, 0, 0);
    assert_eq!(out.len(), 1);
}

#[test]
fn bipred_candidates_never_contribute() {
    let (t, mut c, m) = one_b64(85);
    for cand in c.iter_mut() {
        cand.direction = 2; // bi
    }
    let v = view(&t, &c, &m, 85);
    let out = correspondence_from_mvs(&v, &geom(64, 64), CorrespondenceMethod::Mv32x32, 0, 0);
    assert!(out.is_empty(), "bipred must be skipped");
}

#[test]
fn list1_candidates_match_on_the_list1_fields() {
    let (t, mut c, m) = one_b64(85);
    for cand in c.iter_mut() {
        cand.direction = 1;
        cand.ref1_list = 1;
        cand.ref_idx_l1 = 2;
        // The list-0 fields say something else entirely; they must be
        // ignored for a direction-1 candidate.
        cand.ref0_list = 0;
        cand.ref_idx_l0 = 0;
    }
    let v = view(&t, &c, &m, 85);
    assert!(
        correspondence_from_mvs(&v, &geom(64, 64), CorrespondenceMethod::Mv64x64, 0, 0).is_empty(),
        "a direction-1 candidate must not match on the list-0 fields"
    );
    assert_eq!(
        correspondence_from_mvs(&v, &geom(64, 64), CorrespondenceMethod::Mv64x64, 1, 2).len(),
        1
    );
}

/// The remap cascade: with 8x8 ME off, an 8x8-tier index is remapped to
/// the 16x16 tier; with 16x16 ALSO off it is remapped again to 32x32.
#[test]
fn index_remap_cascades_when_both_me_tiers_are_off() {
    let (t, c, m) = one_b64(85);
    let mut g = geom(64, 64);

    // Everything on: MV_8x8 index i maps to n_idx = 21 + i.
    let v = view(&t, &c, &m, 85);
    let on = correspondence_from_mvs(&v, &g, CorrespondenceMethod::Mv8x8, 0, 0);
    assert_eq!(on.len(), 64);
    assert_eq!(i32::from(m[21].x), on[0].rx - on[0].x);

    // 8x8 off: n_idx 21 remaps through ME_IDX_85_8X8_TO_16X16[0].
    g.enable_me_8x8 = false;
    let off8 = correspondence_from_mvs(&v, &g, CorrespondenceMethod::Mv8x8, 0, 0);
    let want8 = usize::from(ME_IDX_85_8X8_TO_16X16[0]);
    assert_eq!(i32::from(m[want8].x), off8[0].rx - off8[0].x);
    assert_ne!(
        off8[0].rx, on[0].rx,
        "the remap must actually change the MV"
    );

    // 16x16 off as well: if the once-remapped index is still >= 5 it
    // remaps AGAIN. Assert against the doubly-remapped index, which is
    // what a port doing only the first remap would get wrong.
    g.enable_me_16x16 = false;
    let off16 = correspondence_from_mvs(&v, &g, CorrespondenceMethod::Mv8x8, 0, 0);
    let mut want16 = want8 as u8;
    if want16 >= MAX_SB64_PU_COUNT_WO_16X16 {
        want16 = ME_IDX_16X16_TO_PARENT_32X32[(want16 - MAX_SB64_PU_COUNT_WO_16X16) as usize];
    }
    assert_eq!(
        i32::from(m[usize::from(want16)].x),
        off16[0].rx - off16[0].x
    );
}

/// The downsample shift is applied to all four coordinates AFTER the MV is
/// added, so `rx` is `(bx + mv.x) >> shift`, not `(bx >> shift) + mv.x`.
#[test]
fn downsample_shift_is_applied_after_adding_the_mv() {
    // TWO b64s wide, so the buffers must cover both.
    let totals = vec![1u8; 2 * 85];
    let cands = vec![MeCandidateRef::default(); 2 * 85];
    let mut mvs = vec![Mv { x: 0, y: 0 }; 2 * 85];
    // odd MV, so the shift is not distributive over the addition
    mvs[0] = Mv { x: 3, y: 3 };
    mvs[85] = Mv { x: 3, y: 3 };
    let v = view(&totals, &cands, &mvs, 85);
    let mut g = geom(128, 64);
    g.gm_downsample_level = GmDownsampleLevel::Down;
    let out = correspondence_from_mvs(&v, &g, CorrespondenceMethod::Mv64x64, 0, 0);
    assert_eq!(out.len(), 2);
    // b64 at x = 64: (64 + 3) >> 1 == 33, whereas (64 >> 1) + 3 == 35.
    assert_eq!(out[1].x, 32);
    assert_eq!(out[1].rx, 33);
}

#[test]
fn corners_is_refused_not_silently_empty() {
    let (t, c, m) = one_b64(85);
    let v = view(&t, &c, &m, 85);
    assert_eq!(
        gm_compute_correspondence(&v, &geom(64, 64), CorrespondenceMethod::Corners, 0, 0),
        Err(CorrespondenceError::CornersUnported)
    );
    assert!(
        gm_compute_correspondence(&v, &geom(64, 64), CorrespondenceMethod::Mv64x64, 0, 0).is_ok()
    );
}
