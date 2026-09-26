use super::*;

/// Build a 192x256 luma plane of 8x8 intra blocks with pseudo-random
/// pixels and deblock it at `true_w`, returning the filtered plane.
fn deblock_192x256(true_w: usize, true_h: usize) -> alloc::vec::Vec<u8> {
    const W: usize = 192;
    const H: usize = 256;
    let mut geom = DeblockGeom::new(W, H, true_w, true_h);
    for by in (0..H).step_by(8) {
        for bx in (0..W).step_by(8) {
            geom.record_block(bx, by, 8, 8, false, false);
        }
    }
    let mut buf = unfiltered_plane();
    filter_plane(&mut buf, W, W, H, 0, 0, 32, 32, &geom, 0);
    buf
}

/// Flat 8x8 tiles separated by a SMALL step. The deblock mask only fires
/// when the samples either side of an edge are close (`|p1 - p0| <= lim`),
/// so pseudo-random noise filters NOTHING and every assertion below would
/// be vacuous — measured: the anti-vacuity arm caught exactly that.
fn unfiltered_plane() -> alloc::vec::Vec<u8> {
    const W: usize = 192;
    const H: usize = 256;
    let mut buf: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(W * H);
    for r in 0..H {
        for c in 0..W {
            buf.push((100 + ((c / 8) * 3 + (r / 8) * 5) % 17) as u8);
        }
    }
    buf
}

/// AV1 spec 7.14.5 `onScreen` / C `set_lpf_parameters`
/// (deblocking_filter.c:225-230): nothing is filtered at or past the TRUE
/// frame extent, even though the mi grid and every buffer run to the
/// 8-ALIGNED extent.
///
/// This is the defect that made `city-lossless 188x256 p2 q33` pick
/// deblock level 13 where C picks 10: the level search measures its SSE
/// over the ALIGNED plane (C `picture_sse_calculations` uses
/// `aligned_width`), so filtering the four padding columns moves the
/// landscape the search hill-climbs.
///
/// ANTI-VACUITY: the same call with `true_w == 192` MUST touch those
/// columns. Without that half, a test that only asserts "columns 188..191
/// are unchanged" would keep passing if `filter_plane` stopped filtering
/// altogether.
#[test]
fn deblock_skips_edges_past_the_true_frame_width() {
    const W: usize = 192;
    const H: usize = 256;
    let unfiltered = unfiltered_plane();
    let clamped = deblock_192x256(188, H);
    let unclamped = deblock_192x256(W, H);

    // Anti-vacuity: at true_w == aligned the pad columns DO get filtered,
    // and the mi unit at x = 188 is the only difference between the runs.
    let unclamped_moved = (0..H)
        .flat_map(|r| (188..W).map(move |c| r * W + c))
        .filter(|&i| unclamped[i] != unfiltered[i])
        .count();
    assert!(
        unclamped_moved > 0,
        "vacuous: filtering never touches columns 188..192 even at true_w == 192"
    );

    // The rule: with true_w = 188, no pixel at or past column 188 moves.
    let bad: alloc::vec::Vec<(usize, usize)> = (0..H)
        .flat_map(|r| (188..W).map(move |c| (r, c)))
        .filter(|&(r, c)| clamped[r * W + c] != unfiltered[r * W + c])
        .collect();
    assert!(
        bad.is_empty(),
        "filtered {} pixels past the true frame width (first {:?})",
        bad.len(),
        bad.first()
    );
    // And the two runs must actually differ (the clamp is live).
    assert_ne!(clamped, unclamped);
}

/// The same guard on the HEIGHT axis (`y >= FrameHeight`), which needs
/// its own case: `lpf_params` tests the two bounds independently and a
/// width-only fix would leave the vertical half open.
#[test]
fn deblock_skips_edges_past_the_true_frame_height() {
    const W: usize = 192;
    const H: usize = 256;
    let unfiltered = unfiltered_plane();
    let clamped = deblock_192x256(W, 252);
    let unclamped = deblock_192x256(W, H);
    let unclamped_moved = (252..H)
        .flat_map(|r| (0..W).map(move |c| r * W + c))
        .filter(|&i| unclamped[i] != unfiltered[i])
        .count();
    assert!(unclamped_moved > 0, "vacuous: rows 252..256 never filtered");
    let bad = (252..H)
        .flat_map(|r| (0..W).map(move |c| (r, c)))
        .filter(|&(r, c)| clamped[r * W + c] != unfiltered[r * W + c])
        .count();
    assert_eq!(bad, 0, "filtered {bad} pixels past the true frame height");
}

/// Hand-computed values of the C closed form (AC step table x the
/// KEY_FRAME fit): q_step(30) = 34 -> (597142 - 421574 + 131072) >> 18
/// = 1; q_step(50) = 54 -> 657900 >> 18 = 2; q_step(63) = 70 ->
/// 938908 >> 18 = 3. Chroma = y / 2 (truncated before clamp).
#[test]
fn key_frame_levels_match_c_formula() {
    assert_eq!(pick_filter_levels_key_frame(30, 8).levels, [1, 1, 0, 0]);
    assert_eq!(pick_filter_levels_key_frame(50, 8).levels, [2, 2, 1, 1]);
    assert_eq!(pick_filter_levels_key_frame(63, 8).levels, [3, 3, 1, 1]);
    // qindex 0: q_step = 4 -> 70252 - 421574 = negative -> clamps to 0.
    assert_eq!(pick_filter_levels_key_frame(0, 8).levels, [0, 0, 0, 0]);
    // Top of the table: q_step(255) = 1828 -> (32105164 - 421574 +
    // 131072) >> 18 = 121 -> clamps to 63; chroma 121/2 = 60.
    assert_eq!(
        pick_filter_levels_key_frame(255, 8).levels,
        [63, 63, 60, 60]
    );
}

/// bd10 KEY arm (deblocking_filter.c:1070-1084): q = AC_QLOOKUP_10[qidx],
/// filt_guess = ROUND_POWER_OF_TWO(q*20723 + 4060632, 20) - 4, chroma = /2.
#[test]
fn key_frame_levels_bd10_match_c_formula() {
    // qindex 128: AC_QLOOKUP_10[128] = 592 -> (592*20723 + 4060632 +
    // (1<<19)) >> 20 - 4 = (12268016 + 4060632 + 524288)>>20 - 4
    // = 16852936>>20 - 4 = 16 - 4 = 12; chroma 12/2 = 6.
    let q128 = crate::bd10::AC_QLOOKUP_10[128] as i32;
    let expect = ((q128 * 20723 + 4060632 + (1 << 19)) >> 20) - 4;
    let lv = pick_filter_levels_key_frame(128, 10);
    assert_eq!(lv.levels[0] as i32, expect.clamp(0, MAX_LOOP_FILTER));
    assert_eq!(lv.levels[2] as i32, (expect / 2).clamp(0, MAX_LOOP_FILTER));
    // bd8 at the same qindex must be UNCHANGED (byte-neutral guarantee).
    let q8 = svtav1_dsp::quant_tables::AC_QLOOKUP_8[128] as i32;
    let e8 = (q8 * 17563 - 421574 + (1 << 17)) >> 18;
    assert_eq!(
        pick_filter_levels_key_frame(128, 8).levels[0] as i32,
        e8.clamp(0, MAX_LOOP_FILTER)
    );
}

#[test]
fn any_requires_luma() {
    assert!(
        !LfLevels {
            levels: [0, 0, 1, 1]
        }
        .any()
    );
    assert!(
        LfLevels {
            levels: [1, 0, 0, 0]
        }
        .any()
    );
    assert!(LfLevels::default() == LfLevels { levels: [0; 4] });
}
