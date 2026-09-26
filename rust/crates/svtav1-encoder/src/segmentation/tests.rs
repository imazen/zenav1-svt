use super::*;

#[test]
fn log2f_safe_matches_get_msb() {
    // C `svt_log2f_safe(x) = get_msb(x | 1)`; get_msb(n) = 31 - clz(n).
    assert_eq!(log2f_safe(0), 0); // the `| 1` makes 0 legal
    assert_eq!(log2f_safe(1), 0);
    assert_eq!(log2f_safe(2), 1);
    assert_eq!(log2f_safe(3), 1);
    assert_eq!(log2f_safe(255), 7);
    assert_eq!(log2f_safe(256), 8);
    assert_eq!(log2f_safe(65535), 15);
}

#[test]
fn calculate_segmentation_data_hand_vectors() {
    // Trace of segmentation.c:249-260.
    // (a) only ALT_Q on segments 0..4 -> last_active = 4, pre_skip stays 0
    //     (ALT_Q = 0 < SEG_LVL_REF_FRAME = 5).
    let mut seg = SegmentationParams::default();
    for row in seg.feature_enabled.iter_mut().take(5) {
        row[SEG_LVL_ALT_Q] = 1;
    }
    calculate_segmentation_data(&mut seg);
    assert_eq!(seg.last_active_seg_id, 4);
    assert_eq!(seg.seg_id_pre_skip, 0);

    // (b) a REF_FRAME feature on segment 2 sets seg_id_pre_skip, and
    //     last_active_seg_id still tracks the HIGHEST enabled id.
    let mut seg = SegmentationParams::default();
    seg.feature_enabled[2][SEG_LVL_REF_FRAME] = 1;
    seg.feature_enabled[6][SEG_LVL_ALT_Q] = 1;
    calculate_segmentation_data(&mut seg);
    assert_eq!(seg.last_active_seg_id, 6);
    assert_eq!(seg.seg_id_pre_skip, 1);

    // (c) nothing enabled -> both stay at their zero-init values.
    let mut seg = SegmentationParams::default();
    calculate_segmentation_data(&mut seg);
    assert_eq!(seg.last_active_seg_id, 0);
    assert_eq!(seg.seg_id_pre_skip, 0);
}

/// Hand-traced through segmentation.c:262-315 for a uniform frame.
#[test]
fn find_segment_qps_uniform_frame_hand_vector() {
    // One b64 whose 64 8x8 variances are all 100.
    let mut row = [0u16; VARIANCE_BLOCK_COUNT];
    for v in row
        .iter_mut()
        .take(ME_TIER_ZERO_PU_8X8_63 + 1)
        .skip(ME_TIER_ZERO_PU_8X8_0)
    {
        *v = 100;
    }
    let variance = [row];

    // C trace: min_var = max_var = 100; local_avg = 6400; avg_var =
    // 6400 >> 6 = 100; /1 = 100; log2f_safe(100) = 6.
    // min_var_log = max_var_log = 6; diff = 0 <= 8 -> step_size = 1.
    // bin_edge = 7, bin_center = 3.
    // i=7: edge = 1<<7 = 128, offset = 2*(max(1,3) - 6) = -6
    // i=6: edge = 1<<8 = 256, offset = 2*(4-6) = -4
    // i=5: 1<<9  = 512,  2*(5-6)  = -2
    // i=4: 1<<10 = 1024, 2*(6-6)  = 0
    // i=3: 1<<11 = 2048, 2*(7-6)  = 2
    // i=2: 1<<12 = 4096, 2*(8-6)  = 4
    // i=1: 1<<13 = 8192, 2*(9-6)  = 6
    // i=0: 1<<14 = 16384, 2*(10-6) = 8
    let mut seg = SegmentationParams::default();
    find_segment_qps(&mut seg, &variance, 1);
    assert_eq!(
        seg.variance_bin_edge,
        [16384, 8192, 4096, 2048, 1024, 512, 256, 128]
    );
    let q: [i16; 8] = core::array::from_fn(|i| seg.feature_data[i][SEG_LVL_ALT_Q]);
    assert_eq!(q, [8, 6, 4, 2, 0, -2, -4, -6]);
}

/// The `int16_t` truncation of `POW2(bin_edge)` (segmentation.c:292).
#[test]
fn find_segment_qps_bin_edge_truncates_to_i16() {
    // Push min_var_log up so the top bins exceed bit 15.
    let mut row = [0u16; VARIANCE_BLOCK_COUNT];
    for v in row
        .iter_mut()
        .take(ME_TIER_ZERO_PU_8X8_63 + 1)
        .skip(ME_TIER_ZERO_PU_8X8_0)
    {
        *v = 60000;
    }
    let variance = [row];
    // log2f_safe(60000) = 15 for min/max; local_avg = 3_840_000,
    // >>6 = 60000; avg_var = 60000 -> log2f_safe = 15.
    // diff = 0 -> step 1; bin_edge = 16, bin_center = 8.
    // i=7: 1<<16 = 65536 -> (int16_t)65536 == 0; offset 2*(8-15) = -14.
    // i=6: 1<<17 -> 0; 2*(9-15)  = -12
    // i=5: 1<<18 -> 0; 2*(10-15) = -10
    // i=4: 1<<19 -> 0; 2*(11-15) =  -8
    // i=3: 1<<20 -> 0; 2*(12-15) =  -6
    // i=2: 1<<21 -> 0; 2*(13-15) =  -4
    // i=1: 1<<22 -> 0; 2*(14-15) =  -2
    // i=0: 1<<23 -> 0; 2*(15-15) =   0 (not < 0, so no clamp fires)
    let mut seg = SegmentationParams::default();
    find_segment_qps(&mut seg, &variance, 1);
    assert_eq!(seg.variance_bin_edge, [0; MAX_SEGMENTS]);
    let q: [i16; 8] = core::array::from_fn(|i| seg.feature_data[i][SEG_LVL_ALT_Q]);
    assert_eq!(q, [0, -2, -4, -6, -8, -10, -12, -14]);
    // Every bin edge truncated to 0, so `variance <= edge` is false for
    // any nonzero variance and apply_segmentation... falls back to 0.
    let mut probe = [0u16; VARIANCE_BLOCK_COUNT];
    probe[ME_TIER_ZERO_PU_64X64] = 1;
    assert_eq!(
        apply_segmentation_based_quantization(&seg, &probe, BlockSize::Block64x64, 0, 0, 200),
        0
    );
}

#[test]
fn setup_segmentation_disabled_unless_aq_mode_1() {
    let variance = [[0u16; VARIANCE_BLOCK_COUNT]; 1];
    for aq in [0u8, 2, 3] {
        let mut seg = SegmentationParams::default();
        setup_segmentation(&mut seg, aq, &variance, 1);
        assert!(!seg.segmentation_enabled, "aq_mode {aq} must not enable");
        assert_eq!(seg.feature_enabled, [[0; SEG_LVL_MAX]; MAX_SEGMENTS]);
    }
    let mut seg = SegmentationParams::default();
    setup_segmentation(&mut seg, 1, &variance, 1);
    assert!(seg.segmentation_enabled);
    assert!(seg.segmentation_update_map && seg.segmentation_update_data);
    assert!(!seg.segmentation_temporal_update);
    // ALT_Q enabled on every segment, nothing else.
    for i in 0..MAX_SEGMENTS {
        assert_eq!(seg.feature_enabled[i][SEG_LVL_ALT_Q], 1);
        for j in 1..SEG_LVL_MAX {
            assert_eq!(seg.feature_enabled[i][j], 0);
        }
    }
    // ALT_Q < SEG_LVL_REF_FRAME, so seg_id_pre_skip stays 0 and
    // last_active_seg_id is the top segment.
    assert_eq!(seg.last_active_seg_id, (MAX_SEGMENTS - 1) as u8);
    assert_eq!(seg.seg_id_pre_skip, 0);
}

#[test]
fn get_variance_for_cu_index_derivations() {
    let mut var = [0u16; VARIANCE_BLOCK_COUNT];
    // Tag each entry with its index so the assert names the index picked.
    for (i, v) in var.iter_mut().enumerate() {
        *v = (i * 2) as u16;
    }
    // 8x8 at (16, 24): index = 21 + (16>>3) + 24 = 21 + 2 + 24 = 47.
    assert_eq!(
        get_variance_for_cu(BlockSize::Block8x8, 16, 24, &var),
        var[47]
    );
    // 16x16 at (32, 16): 5 + (32>>4) + (16>>2) = 5 + 2 + 4 = 11.
    assert_eq!(
        get_variance_for_cu(BlockSize::Block16x16, 32, 16, &var),
        var[11]
    );
    // 32x32 at (32, 32): 1 + (32>>5) + (32>>4) = 1 + 1 + 2 = 4.
    assert_eq!(
        get_variance_for_cu(BlockSize::Block32x32, 32, 32, &var),
        var[4]
    );
    // 64x64 (and every 128 size) -> index 0, twice.
    assert_eq!(
        get_variance_for_cu(BlockSize::Block64x64, 0, 0, &var),
        var[0]
    );
    // 8x16 at (0, 8): index0 = 21 + 0 + 8 = 29, index1 = 30 (the C
    // dimension swap — the vertical pair takes its HORIZONTAL neighbour).
    assert_eq!(
        get_variance_for_cu(BlockSize::Block8x16, 0, 8, &var),
        var_avg2(var[29], var[30])
    );
    // 16x8 at (0, 8): index0 = 29, index1 = 29 + org_y = 37.
    assert_eq!(
        get_variance_for_cu(BlockSize::Block16x8, 0, 8, &var),
        var_avg2(var[29], var[37])
    );
    // 16x8 at the top SB row: index1 == index0 (org_y == 0).
    assert_eq!(
        get_variance_for_cu(BlockSize::Block16x8, 8, 0, &var),
        var[22]
    );
}

#[test]
fn apply_segmentation_picks_highest_covering_non_lossless_bin() {
    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        variance_bin_edge: [16384, 8192, 4096, 2048, 1024, 512, 256, 128],
        ..SegmentationParams::default()
    };
    for (i, off) in [8i16, 6, 4, 2, 0, -2, -4, -6].iter().enumerate() {
        seg.feature_data[i][SEG_LVL_ALT_Q] = *off;
    }
    let mut var = [0u16; VARIANCE_BLOCK_COUNT];
    var[0] = 100; // 64x64 reads index 0
    // variance 100 <= 128 (segment 7) and base 40 + (-6) = 34 > 0 -> id 7.
    assert_eq!(
        apply_segmentation_based_quantization(&seg, &var, BlockSize::Block64x64, 0, 0, 40),
        7
    );
    // At base 5 the low segments would go lossless: 5-6 = -1, 5-4 = 1 > 0
    // -> the walk skips 7 and takes 6.
    assert_eq!(
        apply_segmentation_based_quantization(&seg, &var, BlockSize::Block64x64, 0, 0, 5),
        6
    );
    // variance above every bin edge -> nothing matches -> the C
    // pre-loop initialization of 0 stands.
    var[0] = 40000;
    assert_eq!(
        apply_segmentation_based_quantization(&seg, &var, BlockSize::Block64x64, 0, 0, 40),
        0
    );
}

#[test]
fn roi_map_sb64_reads_the_owning_b64_cell() {
    let map = [0u8, 1, 2, 3, 4, 5, 6, 7];
    let roi = RoiMapEvt {
        b64_seg_map: &map,
        seg_qp: [0; MAX_SEGMENTS],
        max_seg_id: 7,
    };
    let seg = SegmentationParams {
        segmentation_enabled: true,
        ..SegmentationParams::default()
    };
    // No offsets -> base 40 keeps every segment non-lossless, so the walk
    // stops immediately at the mapped id.
    // stride_b64 = 4, SB at (128, 64) -> col 2, row 1 -> map[1*4+2] = 6.
    assert_eq!(
        roi_map_apply_segmentation_based_quantization(
            &seg,
            &roi,
            4,
            false,
            128,
            64,
            BlockSize::Block64x64,
            0,
            0,
            40,
            0, // incoming segment_id (irrelevant: the walk finds a segment)
        ),
        6
    );
}

/// C's downward walk (segmentation.c:121-129) only WRITES
/// `blk_ptr->segment_id` when it finds a segment whose
/// `base_q_idx + ALT_Q > 0`. If every candidate from the mapped id down to
/// 0 is lossless it falls through, leaving the INCOMING value in place —
/// and C's assert at :131-133 then fires in a debug build while a release
/// build ships that value.
///
/// The port reproduces BOTH halves, which is why this test is
/// `should_panic`: the `debug_assert!` mirrors C's assert (so a debug build
/// stops in the same place C does), and the value it would have returned in
/// release is now the incoming id rather than a fabricated 0.
///
/// The return-value half cannot be asserted from a debug test build for
/// exactly that reason; it is pinned by construction instead —
/// `let mut out: u8 = incoming_segment_id;` is the loop's initial value, so
/// a fall-through returns it. An earlier revision initialised `out` to 0
/// and would have shipped 0 here.
/// Debug-only BY CONSTRUCTION: it asserts that a `debug_assert!` fires, and
/// `debug_assert!` is compiled out in release — so under `--release` this
/// test cannot panic and `should_panic` fails. That is not a flake and not
/// a reason to weaken it; the behaviour it pins genuinely does not exist in
/// a release build, mirroring C, which is the whole point of the test.
///
/// The gate is `cfg(debug_assertions)` rather than `#[ignore]` so the
/// decision is visible in the source and the test cannot silently
/// self-skip. The release half — that the fall-through returns the INCOMING
/// id rather than a fabricated 0 — is covered by
/// [`roi_map_all_lossless_walk_returns_incoming_id`] below, which is
/// release-only for the mirror-image reason: in a debug build that same
/// call panics on the assert before it can return. Between the two, every
/// build configuration exercises this path.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "asserts the chosen segment is not lossless")]
fn roi_map_all_lossless_walk_asserts_exactly_where_c_does() {
    let map = [3u8, 3, 3, 3, 3, 3, 3, 3];
    let roi = RoiMapEvt {
        b64_seg_map: &map,
        seg_qp: [0; MAX_SEGMENTS],
        max_seg_id: 7,
    };
    // Every segment carries an ALT_Q driving base 4 to <= 0, so no
    // candidate in 3..=0 qualifies and C never assigns.
    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        ..SegmentationParams::default()
    };
    for row in seg.feature_data.iter_mut() {
        row[SEG_LVL_ALT_Q] = -4;
    }
    let _ = roi_map_apply_segmentation_based_quantization(
        &seg,
        &roi,
        4,
        false,
        128,
        64,
        BlockSize::Block64x64,
        0,
        0,
        4,
        7, // the incoming blk_ptr->segment_id
    );
}

/// The release half of [`roi_map_all_lossless_walk_asserts_exactly_where_c_does`].
///
/// C's downward walk falls through without writing when every candidate is
/// lossless, so a release build ships the INCOMING `segment_id`. The port
/// must do the same — `let mut out: u8 = incoming_segment_id;` is the
/// loop's initial value. An earlier revision initialised `out` to 0 and
/// would have shipped 0 here, silently disagreeing with C on exactly the
/// input where C's own assert says the situation is degenerate.
///
/// Release-only: in a debug build the `debug_assert!` fires before the
/// function can return, which is what the companion test pins.
#[cfg(not(debug_assertions))]
#[test]
fn roi_map_all_lossless_walk_returns_incoming_id() {
    const INCOMING: u8 = 7;
    let map = [3u8, 3, 3, 3, 3, 3, 3, 3];
    let roi = RoiMapEvt {
        b64_seg_map: &map,
        seg_qp: [0; MAX_SEGMENTS],
        max_seg_id: 7,
    };
    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        ..SegmentationParams::default()
    };
    for row in seg.feature_data.iter_mut() {
        row[SEG_LVL_ALT_Q] = -4;
    }
    let got = roi_map_apply_segmentation_based_quantization(
        &seg,
        &roi,
        4,
        false,
        128,
        64,
        BlockSize::Block64x64,
        0,
        0,
        4,
        INCOMING,
    );
    assert_eq!(
        got, INCOMING,
        "an all-lossless walk must fall through and return the incoming \
             segment id, as C does; returning 0 here would be a fabricated value"
    );
}

#[test]
fn roi_map_sb128_takes_min_over_intersected_quadrants() {
    // 4x2 grid of b64 cells, stride 4.
    let map = [5u8, 3, 9, 9, 7, 2, 9, 9];
    let roi = RoiMapEvt {
        b64_seg_map: &map,
        seg_qp: [0; MAX_SEGMENTS],
        max_seg_id: 7,
    };
    let seg = SegmentationParams {
        segmentation_enabled: true,
        ..SegmentationParams::default()
    };
    // SB128 at (0,0): quadrants are (0,0),(64,0),(0,64),(64,64) ->
    // map[0]=5, map[1]=3, map[4]=7, map[5]=2.
    // A 128x128 block covers all four -> min = 2.
    assert_eq!(
        roi_map_apply_segmentation_based_quantization(
            &seg,
            &roi,
            4,
            true,
            0,
            0,
            BlockSize::Block128x128,
            0,
            0,
            40,
            0, // incoming segment_id (irrelevant: the walk finds a segment)
        ),
        2
    );
    // A 64x64 block at SB-relative (0,0) covers only the first quadrant.
    assert_eq!(
        roi_map_apply_segmentation_based_quantization(
            &seg,
            &roi,
            4,
            true,
            0,
            0,
            BlockSize::Block64x64,
            0,
            0,
            40,
            0, // incoming segment_id (irrelevant: the walk finds a segment)
        ),
        5
    );
    // A 64x64 block at SB-relative (64,64) covers only the last quadrant.
    assert_eq!(
        roi_map_apply_segmentation_based_quantization(
            &seg,
            &roi,
            4,
            true,
            0,
            0,
            BlockSize::Block64x64,
            64,
            64,
            40,
            0, // incoming segment_id (irrelevant: the walk finds a segment)
        ),
        2
    );
}

#[test]
fn roi_map_setup_enables_alt_q_and_four_lf_features() {
    let map = [0u8; 4];
    let roi = RoiMapEvt {
        b64_seg_map: &map,
        seg_qp: [-20, -10, 0, 10, 20, 0, 0, 0],
        max_seg_id: 4,
    };
    let mut seg = SegmentationParams::default();
    roi_map_setup_segmentation(&mut seg, &roi, 120, 8);
    assert!(seg.segmentation_enabled);
    assert!(seg.segmentation_update_map && seg.segmentation_update_data);
    assert!(!seg.segmentation_temporal_update);
    for i in 0..=4usize {
        assert_eq!(seg.feature_enabled[i][SEG_LVL_ALT_Q], 1);
        assert_eq!(seg.feature_enabled[i][SEG_LVL_ALT_LF_Y_V], 1);
        assert_eq!(seg.feature_enabled[i][SEG_LVL_ALT_LF_V], 1);
        assert_eq!(seg.feature_data[i][SEG_LVL_ALT_Q], roi.seg_qp[i]);
    }
    // segments above max_seg_id untouched
    for i in 5..MAX_SEGMENTS {
        assert_eq!(seg.feature_enabled[i], [0; SEG_LVL_MAX]);
    }
    // last_active tracks the highest enabled id; ALT_LF_* are all < 5 so
    // seg_id_pre_skip stays 0.
    assert_eq!(seg.last_active_seg_id, 4);
    assert_eq!(seg.seg_id_pre_skip, 0);

    // The deltas are (level at the segment's qindex) - (level at base),
    // which for a NEGATIVE offset (softer q) must be <= 0.
    let base = crate::deblock::pick_filter_levels_key_frame(120, 8).levels;
    let lo = crate::deblock::pick_filter_levels_key_frame(100, 8).levels;
    assert_eq!(
        seg.feature_data[0][SEG_LVL_ALT_LF_Y_V],
        i32::from(lo[0]) as i16 - i32::from(base[0]) as i16
    );
    // segment 2 has offset 0 -> zero delta on every plane
    assert_eq!(seg.feature_data[2][SEG_LVL_ALT_LF_Y_V], 0);
    assert_eq!(seg.feature_data[2][SEG_LVL_ALT_LF_U], 0);
}

#[test]
fn seg_qp_offset_and_apply_seg_qindex() {
    let mut seg = SegmentationParams::default();
    seg.feature_data[3][SEG_LVL_ALT_Q] = -30;
    // disabled -> 0 regardless of the table
    assert_eq!(seg_qp_offset(&seg, 3), 0);
    seg.segmentation_enabled = true;
    assert_eq!(seg_qp_offset(&seg, 3), -30);
    // full_loop.c:1671 only clamps when the offset is nonzero.
    assert_eq!(apply_seg_qindex(300, 0), 300);
    assert_eq!(apply_seg_qindex(250, 30), 255);
    assert_eq!(apply_seg_qindex(10, -30), 0);
}

#[test]
fn segment_filter_level_applies_lut_and_clamps() {
    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        ..SegmentationParams::default()
    };
    seg.feature_enabled[1][SEG_LVL_ALT_LF_Y_H] = 1;
    seg.feature_data[1][SEG_LVL_ALT_LF_Y_H] = -10;
    seg.feature_enabled[1][SEG_LVL_ALT_LF_U] = 1;
    seg.feature_data[1][SEG_LVL_ALT_LF_U] = 40;
    // plane 0 / dir 1 -> SEG_LVL_ALT_LF_Y_H
    assert_eq!(segment_filter_level(&seg, 0, 1, 32, 1), 22);
    // plane 0 / dir 0 -> SEG_LVL_ALT_LF_Y_V, not enabled -> unchanged
    assert_eq!(segment_filter_level(&seg, 0, 0, 32, 1), 32);
    // plane 1 uses SEG_LVL_ALT_LF_U for BOTH directions, and clamps at 63
    assert_eq!(segment_filter_level(&seg, 1, 0, 32, 1), 63);
    assert_eq!(segment_filter_level(&seg, 1, 1, 32, 1), 63);
    // a different segment id has nothing enabled
    assert_eq!(segment_filter_level(&seg, 0, 1, 32, 2), 32);
}

#[test]
fn derive_lossless_matches_md_config_process() {
    // (1) Segmentation DISABLED, base_q_idx > 0: nothing lossless.
    // md_config_process.c:1015-1017 sets coded_lossless = lossless[0] =
    // !base_q_idx, so a nonzero base gives all-false / false.
    let mut seg = SegmentationParams::default();
    assert_eq!(
        derive_lossless(&mut seg, 30),
        ([false; MAX_SEGMENTS], false)
    );

    // (2) Segmentation DISABLED at base_q_idx == 0 — the plain `--qp 0`
    // lossless entry. C sets BOTH coded_lossless and lossless[0] TRUE here.
    // The first revision of this port returned all-false/false, which would
    // have made a future lossless wiring skip its own envelope.
    let mut seg = SegmentationParams::default();
    let (lossless, coded) = derive_lossless(&mut seg, 0);
    assert!(
        coded,
        "base_q_idx 0 with segmentation off IS coded_lossless"
    );
    assert!(lossless[0], "and lossless[0] is set with it");
    assert_eq!(lossless[1..], [false; MAX_SEGMENTS - 1][..]);

    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        ..Default::default()
    };
    for (i, off) in [8i16, 6, 4, 2, 0, -2, -4, -6].iter().enumerate() {
        seg.feature_data[i][SEG_LVL_ALT_Q] = *off;
    }
    // (3) base 10: every segment lands > 0 -> nothing lossless, and since
    // has_lossless_segment is false the auto-disable does NOT fire.
    let mut s10 = seg;
    let (lossless, coded) = derive_lossless(&mut s10, 10);
    assert_eq!(lossless, [false; MAX_SEGMENTS]);
    assert!(!coded);
    assert!(
        s10.segmentation_enabled,
        "no lossless segment -> segmentation must stay enabled"
    );

    // (4) THE MIXED-LOSSLESS AUTO-DISABLE (md_config_process.c:1011-1013).
    // base 5: 5 + (-6) = -1 <= 0, so segment 7 alone is lossless while the
    // rest are lossy. C cannot code that frame, so it turns segmentation
    // OFF -- and then :1015-1017 re-derives from base_q_idx, which is 5,
    // giving all-false / false. Omitting the auto-disable left the port
    // reporting segment 7 lossless AND segmentation enabled.
    let mut s5 = seg;
    let (lossless, coded) = derive_lossless(&mut s5, 5);
    assert!(
        !s5.segmentation_enabled,
        "a frame with both lossless and lossy segments must disable segmentation"
    );
    assert_eq!(
        lossless, [false; MAX_SEGMENTS],
        "after the auto-disable, :1015-1017 re-derives from base_q_idx = 5"
    );
    assert!(!coded);

    // (5) base 0 with all-zero offsets: EVERY segment is lossless, so
    // coded_lossless is true and the auto-disable does NOT fire (it needs
    // !coded_lossless). Segmentation stays enabled.
    let mut all_zero = SegmentationParams {
        segmentation_enabled: true,
        ..SegmentationParams::default()
    };
    let (lossless, coded) = derive_lossless(&mut all_zero, 0);
    assert_eq!(lossless, [true; MAX_SEGMENTS]);
    assert!(coded);
    assert!(
        all_zero.segmentation_enabled,
        "a fully-lossless frame keeps segmentation enabled"
    );
}
