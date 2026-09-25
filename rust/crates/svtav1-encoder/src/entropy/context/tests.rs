use super::*;

#[test]
fn frame_context_default() {
    let fc = FrameContext::new_default();
    // Skip CDF should be initialized with spec defaults
    assert!(fc.skip_cdf[0][0] > 0);
    assert_eq!(fc.skip_cdf[0][0], 1097); // Spec default
    // Partition CDF from spec — cumulative, monotonically increasing
    assert_eq!(fc.partition_cdf[0][0], 13636);
    assert!(fc.partition_cdf[0][0] > fc.partition_cdf[0][1]);
    // KF Y-mode CDF should have proper values
    assert_eq!(fc.kf_y_mode_cdf[0][0][0], 17180);
    // filter_intra_cdfs from the generated C defaults (BLOCK_8X8 row
    // = 24902, BLOCK_32X32 row = 10425 — default_cdfs.rs, drift-tested
    // vs FcTable::FilterIntra in tests/c_parity.rs).
    assert_eq!(fc.filter_intra_cdfs[3][0], 24902);
    assert_eq!(fc.filter_intra_cdfs[9][0], 10425);
}

/// block_size_index must reproduce the C BlockSize enum order
/// (definitions.h:923-946) — spot-check every entry via the
/// C `block_size_wide/high` semantics.
#[test]
fn block_size_index_matches_c_enum_order() {
    const DIMS: [(usize, usize); 22] = [
        (4, 4),
        (4, 8),
        (8, 4),
        (8, 8),
        (8, 16),
        (16, 8),
        (16, 16),
        (16, 32),
        (32, 16),
        (32, 32),
        (32, 64),
        (64, 32),
        (64, 64),
        (64, 128),
        (128, 64),
        (128, 128),
        (4, 16),
        (16, 4),
        (8, 32),
        (32, 8),
        (16, 64),
        (64, 16),
    ];
    for (i, &(w, h)) in DIMS.iter().enumerate() {
        assert_eq!(block_size_index(w, h), i, "{w}x{h}");
    }
}

/// use_filter_intra = 0 through the default BLOCK_8X8 CDF must code
/// the same arithmetic as any nsyms=2 bool with f = icdf[0] (the C
/// aom_write_symbol nsyms==2 specialization) and must adapt the CDF.
#[test]
fn write_use_filter_intra_smoke() {
    let mut fc = FrameContext::new_default();
    let mut w = AomWriter::new(64);
    let before = fc.filter_intra_cdfs[3];
    write_use_filter_intra(&mut w, &mut fc, 3, false);
    assert_ne!(
        fc.filter_intra_cdfs[3], before,
        "CDF must adapt after coding (decoder updates too)"
    );
    let _ = w.done();
}

#[test]
fn frame_context_clone() {
    let fc1 = FrameContext::new_default();
    let fc2 = fc1.clone();
    assert_eq!(fc1.skip_cdf[0][0], fc2.skip_cdf[0][0]);
}

#[test]
fn write_skip_flag() {
    let mut w = AomWriter::new(256);
    let mut fc = FrameContext::new_default();
    write_skip(&mut w, &mut fc, 0, true);
    write_skip(&mut w, &mut fc, 1, false);
    let output = w.done();
    assert!(!output.is_empty());
}

#[test]
fn write_mv_both_signs() {
    let mut w = AomWriter::new(256);
    write_mv_component(&mut w, 42);
    write_mv_component(&mut w, -42);
    let output = w.done();
    assert!(!output.is_empty());
}

#[test]
fn write_intra_mode_range() {
    let mut w = AomWriter::new(256);
    for mode in 0..13 {
        write_intra_mode(&mut w, mode);
    }
    let output = w.done();
    assert!(!output.is_empty());
}

/// Pin tx_size_cat / tx_max_depth against hand-walked C values:
/// bsize_to_tx_size_cat / bsize_to_max_depth chains through
/// eb_sub_tx_size_map from blocksize_to_txsize[bsize]
/// (TX_64X64→TX_32X32→TX_16X16→TX_8X8→TX_4X4; rect TXs halve the
/// larger dim, e.g. TX_32X64→TX_32X32, TX_4X16→TX_4X8→TX_4X4).
#[test]
fn tx_size_cat_and_depth_match_c_tables() {
    // (w, h, cat, max_depth) — every signaling bsize <= 64x64.
    const CASES: [(usize, usize, usize, usize); 18] = [
        (4, 8, 0, 1),
        (8, 4, 0, 1),
        (8, 8, 0, 1),
        (8, 16, 1, 2),
        (16, 8, 1, 2),
        (4, 16, 1, 2),
        (16, 4, 1, 2),
        (16, 16, 1, 2),
        (16, 32, 2, 2),
        (32, 16, 2, 2),
        (8, 32, 2, 2),
        (32, 8, 2, 2),
        (32, 32, 2, 2),
        (32, 64, 3, 2),
        (64, 32, 3, 2),
        (16, 64, 3, 2),
        (64, 16, 3, 2),
        (64, 64, 3, 2),
    ];
    for (w, h, cat, maxd) in CASES {
        assert_eq!(tx_size_cat(w, h), cat, "cat {w}x{h}");
        assert_eq!(tx_max_depth(w, h), maxd, "max_depth {w}x{h}");
    }
}

/// The 64x64 depth-0 tx_depth symbol must come from tx_size_cdf[3][0]
/// with the C default icdf [26986, 21293] (op 4 of the C uniform-p13
/// identity trace: `W CDF nsyms=3 s=0 icdf=[26986,21293,0]`).
#[test]
fn tx_depth_64x64_uses_cat3_defaults() {
    let fc = FrameContext::new_default();
    assert_eq!(tx_size_cat(64, 64), 3);
    assert_eq!(tx_max_depth(64, 64) + 1, 3);
    assert_eq!(&fc.tx_size_cdf[3][0][..2], &[26986, 21293]);
}

// =========================================================================
// Palette PACK writers (task #71 chunk 5). `palette_map_pixel_ctx` is a
// duplicate of `svtav1_encoder::palette::palette_color_index_context`
// (see that fn's doc + docs/palette-port-map.md) — reusing the SAME
// hand-derived vectors from `svtav1-encoder/tests/c_parity_palette.rs`
// here is the cross-check that the duplication stayed faithful.
// =========================================================================

#[test]
fn ceil_log2_pal_matches_c_definition() {
    assert_eq!(ceil_log2_pal(0), 0);
    assert_eq!(ceil_log2_pal(1), 0);
    assert_eq!(ceil_log2_pal(2), 1);
    assert_eq!(ceil_log2_pal(3), 2);
    assert_eq!(ceil_log2_pal(256), 8);
    assert_eq!(ceil_log2_pal(257), 9);
}

#[test]
fn get_unsigned_bits_matches_c_definition() {
    // get_msb(n)+1 for n>0, i.e. floor(log2(n))+1; 0 for n==0.
    assert_eq!(get_unsigned_bits(0), 0);
    assert_eq!(get_unsigned_bits(1), 1);
    assert_eq!(get_unsigned_bits(2), 2);
    assert_eq!(get_unsigned_bits(3), 2);
    assert_eq!(get_unsigned_bits(4), 3);
    assert_eq!(get_unsigned_bits(8), 4);
}

/// C `write_uniform` (entropy_coding.c:4294-4306) hand-computed: n=6
/// (palette size), l=get_unsigned_bits(6)=3, m=(1<<3)-6=2.
#[test]
fn write_uniform_hand_vectors() {
    // v=0 < m=2 -> literal(0, l-1=2): 2 bits "00".
    let mut w = AomWriter::new(64);
    write_uniform(&mut w, 6, 0);
    let out0 = w.done().to_vec();
    // v=1 < m=2 -> literal(1, 2): 2 bits "01".
    let mut w = AomWriter::new(64);
    write_uniform(&mut w, 6, 1);
    let out1 = w.done().to_vec();
    assert_ne!(out0, out1, "distinct v < m must produce distinct bits");
    // v=5 >= m=2 -> literal(m+((v-m)>>1), 2) then literal((v-m)&1, 1)
    // = literal(2+1,2)=literal(3,2) then literal(1,1) — 3 bits total,
    // vs 2 bits for v<m: just check it doesn't panic and emits output.
    let mut w = AomWriter::new(64);
    write_uniform(&mut w, 6, 5);
    assert!(!w.done().is_empty());
    // n=0 (never a real palette size, but write_uniform must no-op
    // rather than panic — l=0 early return).
    let mut w = AomWriter::new(64);
    write_uniform(&mut w, 0, 0);
    let _ = w.done();
}

/// C `delta_encode_palette_colors` (entropy_coding.c:4256-4288)
/// self-consistency: must not panic across the size range and must
/// depend on every input color (changing one color changes the bits).
#[test]
fn write_delta_encoded_colors_hand_consistency() {
    let mut w = AomWriter::new(64);
    write_delta_encoded_colors(&mut w, &[10u16, 12, 20, 21], 8, 1);
    let out_a = w.done().to_vec();
    let mut w = AomWriter::new(64);
    write_delta_encoded_colors(&mut w, &[10u16, 12, 20, 22], 8, 1);
    let out_b = w.done().to_vec();
    assert_ne!(out_a, out_b, "changing a color must change the coded bits");
    // Single-color and empty inputs must not panic (num<=1 early return).
    let mut w = AomWriter::new(64);
    write_delta_encoded_colors(&mut w, &[10u16], 8, 1);
    let _ = w.done();
    let mut w = AomWriter::new(64);
    write_delta_encoded_colors(&mut w, &[], 8, 1);
    let _ = w.done();
}

/// Edge case (exactly one neighbor), all three sub-branches, both
/// orientations — identical vectors to
/// `palette_color_index_context_edge_hand_vectors` in
/// `svtav1-encoder/tests/c_parity_palette.rs` (dropped the trailing
/// `palette_size` arg, which this fn's caller asserts instead).
#[test]
fn palette_map_pixel_ctx_edge_hand_vectors() {
    let map = [10u8, 3, 10, 10];
    assert_eq!(palette_map_pixel_ctx(&map, 4, 0, 1), (0, 4));
    assert_eq!(palette_map_pixel_ctx(&map, 4, 0, 2), (0, 10));
    assert_eq!(palette_map_pixel_ctx(&map, 4, 0, 3), (0, 0));

    let map = [7u8, 2, 7, 7];
    assert_eq!(palette_map_pixel_ctx(&map, 1, 1, 0), (0, 3));
    assert_eq!(palette_map_pixel_ctx(&map, 1, 2, 0), (0, 7));
    assert_eq!(palette_map_pixel_ctx(&map, 1, 3, 0), (0, 0));
}

/// Interior, all three neighbors distinct — identical vectors to
/// `palette_color_index_context_interior_all_distinct_hand_vectors`.
#[test]
fn palette_map_pixel_ctx_interior_all_distinct_hand_vectors() {
    let stride = 2usize;
    let mk = |current: u8| alloc::vec![9, 3, 5, current];
    assert_eq!(palette_map_pixel_ctx(&mk(3), stride, 1, 1), (1, 0));
    assert_eq!(palette_map_pixel_ctx(&mk(7), stride, 1, 1), (1, 8));
    assert_eq!(palette_map_pixel_ctx(&mk(1), stride, 1, 1), (1, 4));
}

/// Interior, left==top only — identical vectors to
/// `palette_color_index_context_interior_left_eq_top_hand_vectors`.
#[test]
fn palette_map_pixel_ctx_interior_left_eq_top_hand_vectors() {
    let stride = 2usize;
    let mk = |current: u8| alloc::vec![9, 4, 4, current];
    assert_eq!(palette_map_pixel_ctx(&mk(4), stride, 1, 1), (3, 0));
    assert_eq!(palette_map_pixel_ctx(&mk(9), stride, 1, 1), (3, 1));
    assert_eq!(palette_map_pixel_ctx(&mk(2), stride, 1, 1), (3, 4));
}

/// Interior, left==topleft only — identical vectors to
/// `palette_color_index_context_interior_left_eq_topleft_hand_vectors`.
#[test]
fn palette_map_pixel_ctx_interior_left_eq_topleft_hand_vectors() {
    let stride = 2usize;
    let mk = |current: u8| alloc::vec![6, 2, 6, current];
    assert_eq!(palette_map_pixel_ctx(&mk(6), stride, 1, 1), (2, 0));
    assert_eq!(palette_map_pixel_ctx(&mk(2), stride, 1, 1), (2, 1));
    assert_eq!(palette_map_pixel_ctx(&mk(9), stride, 1, 1), (2, 9));
}

/// Interior, top==topleft only — identical vectors to
/// `palette_color_index_context_interior_top_eq_topleft_hand_vectors`.
#[test]
fn palette_map_pixel_ctx_interior_top_eq_topleft_hand_vectors() {
    let stride = 2usize;
    let mk = |current: u8| alloc::vec![7, 7, 1, current];
    assert_eq!(palette_map_pixel_ctx(&mk(7), stride, 1, 1), (2, 0));
    assert_eq!(palette_map_pixel_ctx(&mk(1), stride, 1, 1), (2, 1));
    assert_eq!(palette_map_pixel_ctx(&mk(0), stride, 1, 1), (2, 2));
}

/// Interior, all three neighbors equal — identical vectors to
/// `palette_color_index_context_interior_all_equal_hand_vectors`.
#[test]
fn palette_map_pixel_ctx_interior_all_equal_hand_vectors() {
    let stride = 2usize;
    let mk = |current: u8| alloc::vec![5, 5, 5, current];
    assert_eq!(palette_map_pixel_ctx(&mk(5), stride, 1, 1), (4, 0));
    assert_eq!(palette_map_pixel_ctx(&mk(2), stride, 1, 1), (4, 3));
    assert_eq!(palette_map_pixel_ctx(&mk(9), stride, 1, 1), (4, 9));
}

/// `write_palette_mode_info` / `write_palette_map_tokens` end-to-end
/// smoke test: a synthetic 4-color 2x2 map must round-trip through the
/// writer without panicking, and the `Some` arm must code MORE symbols
/// (hence produce different bytes) than the `None` (no-palette) arm on
/// the same block — a cheap shape lock. #71 injection now exercises this
/// end-to-end on the EPICA cell, which is not yet byte-matched (over-
/// picking, #71); this test guards the writer shape independent of that.
/// C's MD-side `sum_intra_stats` reaches `update_palette_cdf` only past
/// its `is_chroma_reference` return (md_rate_estimation.c:760-830): with
/// the chain-sim flag set, a non-chroma-reference block's luma palette
/// symbols leave the rows untouched; a chroma reference, or the real
/// pack (flag clear), adapts them. Both arms pinned, and the coded bytes
/// are identical either way (adaptation is withheld, not the symbols).
#[test]
fn md_side_chroma_gated_palette_withholds_adaptation_only() {
    let base = FrameContext::new_default();
    let coded = |flag: bool, chroma_ref: bool| {
        let mut fc = base.clone();
        fc.md_side_chroma_gated_palette = flag;
        let mut w = AomWriter::new(64);
        // 4x16 DC leaf, no palette: the luma flag is coded (allow_palette
        // holds for 4x16) whether or not the block is a chroma reference.
        write_palette_mode_info(&mut w, &mut fc, 4, 16, 0, 0, chroma_ref, 0, None, 8);
        let bytes = w.done().to_vec();
        (fc.palette_y_mode_cdf[0][0], bytes)
    };
    let (pack, pack_bytes) = coded(false, false);
    let (sim_ref, sim_ref_bytes) = coded(true, true);
    let (sim_nonref, sim_nonref_bytes) = coded(true, false);
    assert_ne!(pack, base.palette_y_mode_cdf[0][0], "the real pack adapts");
    assert_eq!(sim_ref, pack, "a chroma reference adapts in the chain too");
    assert_eq!(
        sim_nonref, base.palette_y_mode_cdf[0][0],
        "a non-chroma-reference block leaves C's MD-side row alone"
    );
    assert_eq!(pack_bytes, sim_ref_bytes);
    assert_eq!(
        pack_bytes, sim_nonref_bytes,
        "only the adaptation is withheld"
    );
}

#[test]
fn write_palette_mode_info_some_vs_none_arm() {
    let mut fc = FrameContext::new_default();
    let mut w = AomWriter::new(256);
    write_palette_mode_info(&mut w, &mut fc, 8, 8, 0, 0, true, 0, None, 8);
    let none_bytes = w.done().to_vec();

    let mut fc2 = FrameContext::new_default();
    let mut w2 = AomWriter::new(256);
    let colors: [u16; 2] = [10, 20];
    let cache_found: [bool; 0] = [];
    let out_of_cache: [u16; 2] = [10, 20];
    write_palette_mode_info(
        &mut w2,
        &mut fc2,
        8,
        8,
        0,
        0,
        true,
        0,
        Some((&colors, &cache_found, &out_of_cache)),
        8,
    );
    // Map: 2x2, 2 colors, anti-diagonal pixels (1,0) and (0,1) each
    // code one context'd symbol on top of the (0,0) write_uniform.
    let map = [0u8, 1, 1, 0];
    write_palette_map_tokens(&mut w2, &mut fc2, &map, 2, 2, 2, 2);
    let some_bytes = w2.done().to_vec();

    assert_ne!(
        none_bytes, some_bytes,
        "palette Some arm must code additional symbols"
    );
}

/// Hand-derived vector for the frame-edge partition CDF gathers.
///
/// `partition_gather_{horz,vert}_alike` are `static INLINE` in a C header
/// (cabac_context_model.h:378-406), so they are NOT reachable through FFI —
/// per the evidence hierarchy they get a hand-computed vector traced from
/// the C source, plus the end-to-end partial-SB identity gate.
///
/// Synthetic 10-symbol inverse CDF with per-symbol Q15 probabilities
/// P = [3000, 4000, 2000, 5000, 1000, 2000, 3000, 4000, 6000, 2768]
/// (sum 32768), stored as `icdf[i] = 32768 - cumulative(i+1)`.
#[test]
fn partition_edge_cdf_gathers_match_c_formula() {
    let icdf: [AomCdfProb; 11] = [
        29768, 25768, 23768, 18768, 17768, 15768, 12768, 8768, 2768, 0, 0,
    ];

    // cdf_element_prob recovers the per-symbol probabilities exactly.
    let mut probs = [0u16; 10];
    for (e, slot) in probs.iter_mut().enumerate() {
        *slot = cdf_element_prob(&icdf, e);
    }
    assert_eq!(
        probs,
        [3000, 4000, 2000, 5000, 1000, 2000, 3000, 4000, 6000, 2768]
    );

    // horz_alike: subtract P(HORZ=1) + P(SPLIT=3) + P(HORZ_A=4)
    //           + P(HORZ_B=5) + P(VERT_A=6) + P(HORZ_4=8)
    //           = 4000+5000+1000+2000+3000+6000 = 21000
    // v = 32768 - 21000 = 11768; out[0] = AOM_ICDF(v) = 32768 - 11768 = 21000
    let mut out = [0 as AomCdfProb; 3];
    partition_gather_horz_alike(&mut out, &icdf, false);
    assert_eq!(out, [21000, 0, 0]);

    // vert_alike: P(VERT=2) + P(SPLIT=3) + P(HORZ_A=4)
    //           + P(VERT_A=6) + P(VERT_B=7) + P(VERT_4=9)
    //           = 2000+5000+1000+3000+4000+2768 = 17768
    // v = 32768 - 17768 = 15000; out[0] = 32768 - 15000 = 17768
    partition_gather_vert_alike(&mut out, &icdf, false);
    assert_eq!(out, [17768, 0, 0]);

    // is_128 drops the *_4 term from each (C `bsize != BLOCK_128X128`).
    partition_gather_horz_alike(&mut out, &icdf, true);
    assert_eq!(out, [15000, 0, 0]);
    partition_gather_vert_alike(&mut out, &icdf, true);
    assert_eq!(out, [15000, 0, 0]);
}

/// The both-false case codes NOTHING (forced SPLIT), and the interior case
/// is bit-identical to the plain `write_partition` path — the property that
/// keeps 64-aligned frames byte-unchanged.
#[test]
fn write_partition_edge_interior_matches_plain_and_offframe_codes_nothing() {
    // Interior (has_rows && has_cols) == plain write_partition.
    let mut fc_a = FrameContext::new_default();
    let mut w_a = AomWriter::new(256);
    write_partition(&mut w_a, &mut fc_a, 5, 3, 10);
    let plain = w_a.done().to_vec();

    let mut fc_b = FrameContext::new_default();
    let mut w_b = AomWriter::new(256);
    write_partition_edge(&mut w_b, &mut fc_b, 5, 3, 10, false, true, true);
    let edge_interior = w_b.done().to_vec();
    assert_eq!(
        plain, edge_interior,
        "interior edge-write must match write_partition"
    );
    assert_eq!(
        fc_a.partition_cdf[5], fc_b.partition_cdf[5],
        "interior edge-write must adapt the CDF identically"
    );

    // Both-false: forced SPLIT, no symbol, and the CDF is NOT adapted.
    let mut fc_c = FrameContext::new_default();
    let before = fc_c.partition_cdf[5];
    let mut w_c = AomWriter::new(256);
    write_partition_edge(&mut w_c, &mut fc_c, 5, 3, 10, false, false, false);
    let nothing = w_c.done().to_vec();
    let mut w_empty = AomWriter::new(256);
    let empty = w_empty.done().to_vec();
    assert_eq!(
        nothing, empty,
        "off-frame quadrant must code no partition symbol"
    );
    assert_eq!(
        before, fc_c.partition_cdf[5],
        "forced-SPLIT must not adapt the CDF"
    );

    // Binary arm: must NOT adapt the persistent CDF (C gathers on the stack).
    let mut fc_d = FrameContext::new_default();
    let before_d = fc_d.partition_cdf[5];
    let mut w_d = AomWriter::new(256);
    write_partition_edge(&mut w_d, &mut fc_d, 5, 3, 10, false, true, false);
    assert_eq!(
        before_d, fc_d.partition_cdf[5],
        "gathered binary arm must leave the frame-context CDF untouched"
    );
}

// ---- Segmentation ----

/// `SegmentationMap::new` must reproduce C's `reset_segmentation_map`
/// (enc_dec_process.c:132-135): `~0`, NOT zero. A zero-filled map would
/// silently make every unwritten neighbour read as segment 0 instead of
/// tripping C's `assert(segment_id < MAX_SEGMENTS)`.
#[test]
fn segmentation_map_resets_to_all_ones() {
    let mut m = SegmentationMap::new(4, 3);
    assert_eq!(m.data, alloc::vec![0xFFu8; 12]);
    m.update(1, 1, 0, 0, 5);
    assert_eq!(m.data[0], 5);
    m.reset();
    assert_eq!(m.data, alloc::vec![0xFFu8; 12]);
}

/// `write_inter_segment_id`'s pre/post-skip routing
/// (entropy_coding.c:4889-4925). The symbol emission it delegates to is
/// byte-pinned against the exported C `write_segment_id` in
/// `svtav1-encoder/tests/c_parity_segmentation.rs`; this asserts only
/// WHICH call site fires.
#[test]
fn write_inter_segment_id_position_routing() {
    let base = SegmentationParams {
        segmentation_enabled: true,
        segmentation_update_map: true,
        segmentation_temporal_update: false,
        segmentation_update_data: true,
        last_active_seg_id: 3,
        ..SegmentationParams::default()
    };
    let run = |seg: &SegmentationParams, pos: SegIdPosition, skip: bool| -> Option<u8> {
        let mut fc = FrameContext::new_default();
        let mut map = SegmentationMap::new(8, 8);
        map.data.fill(0);
        let mut w = AomWriter::new(64);
        write_inter_segment_id(
            &mut w, &mut fc, &mut map, seg, pos, 1, 1, 2, 2, true, true, 2, skip,
        )
    };

    // seg_id_pre_skip = 0 -> only the POST-skip site codes.
    let seg = base;
    assert_eq!(run(&seg, SegIdPosition::PreSkip, false), None);
    assert_eq!(run(&seg, SegIdPosition::PostSkip, false), Some(2));

    // seg_id_pre_skip = 1 -> only the PRE-skip site codes.
    let mut seg = base;
    seg.seg_id_pre_skip = 1;
    assert_eq!(run(&seg, SegIdPosition::PreSkip, false), Some(2));
    assert_eq!(run(&seg, SegIdPosition::PostSkip, false), None);

    // POST-skip with skip=1 takes the inferred-prediction path: nothing
    // is coded and the id becomes the spatial prediction (0 on an
    // all-zero map), not the block's own 2.
    let seg = base;
    assert_eq!(run(&seg, SegIdPosition::PostSkip, true), Some(0));

    // Disabled / no map update -> nothing anywhere.
    let mut seg = base;
    seg.segmentation_enabled = false;
    assert_eq!(run(&seg, SegIdPosition::PostSkip, false), None);
    let mut seg = base;
    seg.segmentation_update_map = false;
    assert_eq!(run(&seg, SegIdPosition::PostSkip, false), None);
    assert_eq!(run(&seg, SegIdPosition::PreSkip, false), None);
}

/// The skip path must not code a symbol AND must stamp the prediction.
#[test]
fn write_segment_id_skip_path_codes_nothing() {
    let mut fc = FrameContext::new_default();
    let mut map = SegmentationMap::new(8, 8);
    map.data.fill(3);
    let cdf_before = fc.spatial_pred_seg_cdf;

    let mut w = AomWriter::new(64);
    let id = write_segment_id(
        &mut w, &mut fc, &mut map, true, 7, 2, 2, 4, 4, true, true, 5, true,
    );
    // Every neighbour is 3, so the prediction is 3 and the block's own 5
    // is discarded.
    assert_eq!(id, 3);
    assert_eq!(map.get_segment_id(1, 1, 4, 4), 3);
    assert_eq!(
        fc.spatial_pred_seg_cdf, cdf_before,
        "no symbol, no adaptation"
    );
}

/// The `avg_cdf_with` refactor (the inline `ndvc` field enumeration
/// became a call to [`crate::inter_mv_code::avg_nmv`]) must be
/// byte-neutral. This replays the PREVIOUS inline enumeration verbatim on
/// a clone and asserts the two agree field-for-field, so the refactor
/// cannot have dropped or reordered a field.
///
/// It also proves the inputs actually differ — averaging two equal
/// contexts is the identity (`avg_cdf_equal_contexts_is_identity`), so a
/// test over equal contexts would pass no matter what the helper did.
#[test]
fn avg_nmv_matches_the_previous_inline_ndvc_enumeration() {
    use crate::entropy::cdf::avg_cdf_entries as avg;
    use crate::entropy::mv_coding::{MvSubpelPrecision, NmvContext};
    use crate::inter_mv_code::update_mv_stats;
    use svtav1_types::motion::Mv;

    // Two DIFFERENT contexts, built by adapting the default through two
    // different MV sequences.
    let mut left = NmvContext::default();
    let mut tr = NmvContext::default();
    for k in 0..40i32 {
        let m = Mv {
            x: (k * 37 % 601 - 300) as i16,
            y: (k * 53 % 401 - 200) as i16,
        };
        update_mv_stats(&mut left, m, Mv::ZERO, MvSubpelPrecision::High);
        let m2 = Mv {
            x: (k * 11 % 257 - 128) as i16,
            y: (k * 97 % 511 - 255) as i16,
        };
        update_mv_stats(&mut tr, m2, Mv::ZERO, MvSubpelPrecision::Low);
    }
    assert_ne!(
        left.joints_cdf, tr.joints_cdf,
        "the two contexts must differ or this test is vacuous"
    );

    // C AVG_CDF_WEIGHT_LEFT / AVG_CDF_WEIGHT_TOP (enc_dec_process.c).
    let (wt_left, wt_tr) = (3i32, 1i32);

    // The PREVIOUS inline enumeration, verbatim.
    let mut want = left.clone();
    avg(&mut want.joints_cdf, &tr.joints_cdf, wt_left, wt_tr);
    for i in 0..2 {
        let (l, r) = (&mut want.comps[i], &tr.comps[i]);
        avg(&mut l.classes_cdf, &r.classes_cdf, wt_left, wt_tr);
        avg(
            l.class0_fp_cdf.as_flattened_mut(),
            r.class0_fp_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
        avg(&mut l.fp_cdf, &r.fp_cdf, wt_left, wt_tr);
        avg(&mut l.sign_cdf, &r.sign_cdf, wt_left, wt_tr);
        avg(&mut l.class0_hp_cdf, &r.class0_hp_cdf, wt_left, wt_tr);
        avg(&mut l.hp_cdf, &r.hp_cdf, wt_left, wt_tr);
        avg(&mut l.class0_cdf, &r.class0_cdf, wt_left, wt_tr);
        avg(
            l.bits_cdf.as_flattened_mut(),
            r.bits_cdf.as_flattened(),
            wt_left,
            wt_tr,
        );
    }
    assert_ne!(
        want.joints_cdf, left.joints_cdf,
        "averaging changed nothing — the probe is inert"
    );

    let mut got = left.clone();
    crate::inter_mv_code::avg_nmv(&mut got, &tr, wt_left, wt_tr);

    assert_eq!(got.joints_cdf, want.joints_cdf, "joints_cdf");
    for i in 0..2 {
        let (g, w) = (&got.comps[i], &want.comps[i]);
        assert_eq!(g.classes_cdf, w.classes_cdf, "comps[{i}].classes_cdf");
        assert_eq!(g.class0_fp_cdf, w.class0_fp_cdf, "comps[{i}].class0_fp_cdf");
        assert_eq!(g.fp_cdf, w.fp_cdf, "comps[{i}].fp_cdf");
        assert_eq!(g.sign_cdf, w.sign_cdf, "comps[{i}].sign_cdf");
        assert_eq!(g.class0_hp_cdf, w.class0_hp_cdf, "comps[{i}].class0_hp_cdf");
        assert_eq!(g.hp_cdf, w.hp_cdf, "comps[{i}].hp_cdf");
        assert_eq!(g.class0_cdf, w.class0_cdf, "comps[{i}].class0_cdf");
        assert_eq!(g.bits_cdf, w.bits_cdf, "comps[{i}].bits_cdf");
    }
}

/// Anti-vacuity for the field above: `avg_cdf_with` must ACTUALLY average
/// `nmvc`, which the inert-on-defaults test cannot show (averaging equal
/// contexts is the identity either way). Give the two neighbours
/// DIFFERENT `nmvc`s and require the result to move.
#[test]
fn avg_cdf_with_actually_averages_nmvc() {
    use crate::entropy::mv_coding::MvSubpelPrecision;
    use crate::inter_mv_code::update_mv_stats;
    use svtav1_types::motion::Mv;

    let mut left = FrameContext::new_default();
    let mut tr = FrameContext::new_default();
    for k in 0..24i32 {
        update_mv_stats(
            &mut left.nmvc,
            // x == 0 on most entries -> mostly MV_JOINT_HZVNZ.
            Mv {
                x: if k % 4 == 0 {
                    (k * 29 % 401 - 200) as i16
                } else {
                    0
                },
                y: (k * 71 % 301 - 149) as i16,
            },
            Mv::ZERO,
            MvSubpelPrecision::High,
        );
    }
    for k in 0..24i32 {
        update_mv_stats(
            &mut tr.nmvc,
            // y == 0 on most entries -> mostly MV_JOINT_HNZVZ.
            Mv {
                x: (k * 13 % 199 - 98) as i16,
                y: if k % 4 == 0 {
                    (k * 41 % 257 - 128) as i16
                } else {
                    0
                },
            },
            Mv::ZERO,
            MvSubpelPrecision::Low,
        );
    }
    assert_ne!(
        left.nmvc.joints_cdf, tr.nmvc.joints_cdf,
        "the two nmvc contexts must differ or this test is vacuous"
    );
    let before = left.nmvc.clone();
    let want = {
        let mut w = left.nmvc.clone();
        crate::inter_mv_code::avg_nmv(&mut w, &tr.nmvc, 3, 1);
        w
    };
    assert_ne!(
        want.joints_cdf, before.joints_cdf,
        "avg_nmv itself changed nothing — the probe is inert"
    );
    left.avg_cdf_with(&tr, 3, 1);
    assert_eq!(
        left.nmvc.joints_cdf, want.joints_cdf,
        "avg_cdf_with did not average nmvc"
    );
    for i in 0..2 {
        assert_eq!(left.nmvc.comps[i].bits_cdf, want.comps[i].bits_cdf);
        assert_eq!(
            left.nmvc.comps[i].class0_fp_cdf,
            want.comps[i].class0_fp_cdf
        );
    }
}

/// The new `nmvc` field must not perturb any current gate: nothing writes
/// an inter MV yet, so both neighbours always hold the DEFAULT context and
/// averaging equal contexts is the identity.
#[test]
fn nmvc_defaults_and_is_inert_under_avg() {
    use crate::entropy::mv_coding::NmvContext;
    let fc = FrameContext::new_default();
    assert_eq!(fc.nmvc.joints_cdf, NmvContext::default().joints_cdf);
    let mut left = FrameContext::new_default();
    let tr = FrameContext::new_default();
    let before = left.nmvc.clone();
    left.avg_cdf_with(&tr, 3, 1);
    assert_eq!(
        left.nmvc.joints_cdf, before.joints_cdf,
        "averaging two default nmvc contexts must be the identity"
    );
    for i in 0..2 {
        assert_eq!(left.nmvc.comps[i].classes_cdf, before.comps[i].classes_cdf);
        assert_eq!(left.nmvc.comps[i].bits_cdf, before.comps[i].bits_cdf);
    }
}
