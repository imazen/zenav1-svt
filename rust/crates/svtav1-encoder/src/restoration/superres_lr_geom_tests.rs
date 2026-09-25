use super::*;

fn pr(unit_size: i32) -> PlaneRest {
    PlaneRest {
        frame_rtype: RESTORE_WIENER,
        unit_size,
        hunits: 0,
        vunits: 0,
        units: alloc::vec::Vec::new(),
    }
}

/// C `svt_av1_loop_restoration_corners_in_sb` (restoration.c:1410) with
/// the superres arms transcribed verbatim — the port must agree with it
/// for every superres denominator, SB position and plane.
///
/// EVIDENCE TIER: hand-transcribed formula, not an FFI call. The C symbol
/// IS exported (`nm -g libSvtAv1Enc.a | grep corners_in_sb`), but it takes
/// an `Av1Common*` whose `child_pcs->rst_info` must be built by hand;
/// shimming it the way `c_parity_intrabc_mvp` shims its context is the
/// upgrade path when superres chunk B.5 wires the rest of the LR path.
fn c_reference(
    unit_size: i32,
    is_uv: bool,
    mi_row: i32,
    mi_col: i32,
    sb_mi: i32,
    upscaled_w: usize,
    frame_h: usize,
    sr_denom: Option<u8>,
) -> Option<(i32, i32, i32, i32)> {
    const SCALE_NUMERATOR: i32 = 8;
    let ss = i32::from(is_uv);
    // C `whole_frame_rect` (restoration.c:51): the LR tile rect is the
    // UPSCALED width, ROUND_POWER_OF_TWO'd for chroma.
    let tile_w = (upscaled_w as i32 + ss) >> ss;
    let tile_h = (frame_h as i32 + ss) >> ss;
    let horz_units = svtav1_dsp::restoration::count_units_in_tile(unit_size, tile_w);
    let vert_units = svtav1_dsp::restoration::count_units_in_tile(unit_size, tile_h);
    let (mi_size_x, mi_size_y) = (4 >> ss, 4 >> ss);
    let unscaled = sr_denom.is_none();
    let mi_to_num_x = if unscaled {
        mi_size_x
    } else {
        mi_size_x * i32::from(sr_denom.unwrap())
    };
    let denom_x = if unscaled {
        unit_size
    } else {
        unit_size * SCALE_NUMERATOR
    };
    let (rnd_x, rnd_y) = (denom_x - 1, unit_size - 1);
    let rcol0 = (mi_col * mi_to_num_x + rnd_x) / denom_x;
    let rrow0 = (mi_row * mi_size_y + rnd_y) / unit_size;
    let rcol1 = (((mi_col + sb_mi) * mi_to_num_x + rnd_x) / denom_x).min(horz_units);
    let rrow1 = (((mi_row + sb_mi) * mi_size_y + rnd_y) / unit_size).min(vert_units);
    (rcol0 < rcol1 && rrow0 < rrow1).then_some((rcol0, rcol1, rrow0, rrow1))
}

#[test]
fn corners_in_sb_matches_c_across_superres_denominators() {
    for &unit_size in &[64i32, 128, 256] {
        for &upscaled_w in &[128usize, 256, 512] {
            let frame_h = 128usize;
            for denom in [None, Some(9u8), Some(12), Some(16)] {
                for is_uv in [false, true] {
                    for sb in 0..(upscaled_w / 64) {
                        let mi_col = (sb * 16) as i32;
                        for mi_row in [0i32, 16, 32] {
                            let got = corners_in_sb(
                                &pr(unit_size),
                                is_uv,
                                mi_row,
                                mi_col,
                                16,
                                upscaled_w,
                                frame_h,
                                denom,
                            );
                            let want = c_reference(
                                unit_size, is_uv, mi_row, mi_col, 16, upscaled_w, frame_h, denom,
                            );
                            assert_eq!(
                                got, want,
                                "unit {unit_size} w {upscaled_w} denom {denom:?} uv {is_uv} \
                                     mi ({mi_row},{mi_col})"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// ANTI-VACUITY: the superres arm must actually change the mapping —
/// otherwise the test above would pass on a port that ignored `sr_denom`.
#[test]
fn superres_shifts_the_restoration_unit_mapping() {
    let (unit_size, upscaled_w, frame_h) = (64i32, 512usize, 128usize);
    let mut differing = 0;
    for sb in 0..(upscaled_w / 64) {
        let mi_col = (sb * 16) as i32;
        let unscaled = corners_in_sb(
            &pr(unit_size),
            false,
            0,
            mi_col,
            16,
            upscaled_w,
            frame_h,
            None,
        );
        let scaled = corners_in_sb(
            &pr(unit_size),
            false,
            0,
            mi_col,
            16,
            upscaled_w,
            frame_h,
            Some(16),
        );
        if unscaled != scaled {
            differing += 1;
        }
    }
    assert!(
        differing > 0,
        "denominator 16 must remap at least one superblock's restoration units"
    );
}
