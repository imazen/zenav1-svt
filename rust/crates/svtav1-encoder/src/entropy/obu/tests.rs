use super::*;
use alloc::vec;

#[test]
fn uleb_encode_small() {
    assert_eq!(uleb_encode(0), vec![0]);
    assert_eq!(uleb_encode(1), vec![1]);
    assert_eq!(uleb_encode(127), vec![127]);
}

/// Expand a `BitWriter`'s payload into one `u8` per bit, MSB first.
fn bits_of(wb: &BitWriter) -> Vec<u8> {
    let data = wb.data();
    (0..wb.bit_len())
        .map(|i| (data[i / 8] >> (7 - (i % 8))) & 1)
        .collect()
}

/// Disabled segmentation must emit EXACTLY the single zero bit the five
/// hardcoded `wb.write_bit(false)` writer sites emit today — that is what
/// makes switching a site over to this function byte-neutral.
#[test]
fn segmentation_params_disabled_is_one_zero_bit() {
    let seg = SegmentationParams::default();
    for primary_ref_none in [true, false] {
        let mut wb = BitWriter::new();
        write_segmentation_params(&mut wb, &seg, primary_ref_none);
        assert_eq!(bits_of(&wb), vec![0u8]);
    }
}

/// The three update flags are coded ONLY when a primary reference exists
/// (C `encode_segmentation`, entropy_coding.c:2251-2257), and the
/// temporal flag only nests under `update_map`.
#[test]
fn segmentation_params_update_flag_gating() {
    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        segmentation_update_map: true,
        segmentation_temporal_update: false,
        segmentation_update_data: false,
        ..SegmentationParams::default()
    };
    // primary_ref_none: no flags at all, and update_data==false skips the
    // feature loop -> one bit total.
    let mut wb = BitWriter::new();
    write_segmentation_params(&mut wb, &seg, true);
    assert_eq!(bits_of(&wb), vec![1u8]);

    // primary ref present: enabled, update_map, temporal_update, update_data.
    let mut wb = BitWriter::new();
    write_segmentation_params(&mut wb, &seg, false);
    assert_eq!(bits_of(&wb), vec![1, 1, 0, 0]);

    // update_map = 0 drops the nested temporal bit.
    seg.segmentation_update_map = false;
    let mut wb = BitWriter::new();
    write_segmentation_params(&mut wb, &seg, false);
    assert_eq!(bits_of(&wb), vec![1, 0, 0]);
}

/// Feature payload widths: `bits[j] + signed[j]` per
/// `svt_aom_segmentation_feature_{bits,signed}`, and `bits == 0` features
/// (SEG_LVL_SKIP / SEG_LVL_GLOBALMV) code their enable bit only.
#[test]
fn segmentation_params_feature_payload_widths() {
    use svtav1_types::segmentation::{
        SEG_LVL_ALT_LF_Y_V, SEG_LVL_ALT_Q, SEG_LVL_GLOBALMV, SEG_LVL_REF_FRAME, SEG_LVL_SKIP,
    };
    let mut seg = SegmentationParams {
        segmentation_enabled: true,
        segmentation_update_data: true,
        ..SegmentationParams::default()
    };
    // Segment 0 only, one feature at a time.
    let cases: [(usize, i16, usize); 5] = [
        (SEG_LVL_ALT_Q, -37, 9),     // su(1+8)
        (SEG_LVL_ALT_LF_Y_V, 21, 7), // su(1+6)
        (SEG_LVL_REF_FRAME, 5, 3),   // f(3), unsigned
        (SEG_LVL_SKIP, 0, 0),        // f(0) — nothing
        (SEG_LVL_GLOBALMV, 0, 0),    // f(0) — nothing
    ];
    for (feature, data, payload_bits) in cases {
        seg.feature_enabled = [[0; SEG_LVL_MAX]; MAX_SEGMENTS];
        seg.feature_data = [[0; SEG_LVL_MAX]; MAX_SEGMENTS];
        seg.feature_enabled[0][feature] = 1;
        seg.feature_data[0][feature] = data;
        let mut wb = BitWriter::new();
        write_segmentation_params(&mut wb, &seg, true);
        let bits = bits_of(&wb);
        // 1 enabled bit + 8*8 enable bits + payload
        assert_eq!(
            bits.len(),
            1 + MAX_SEGMENTS * SEG_LVL_MAX + payload_bits,
            "total width for feature {feature}"
        );
        if payload_bits > 0 {
            let start = 1 + feature + 1;
            let coded: i32 = bits[start..start + payload_bits]
                .iter()
                .fold(0i32, |acc, &b| (acc << 1) | i32::from(b));
            // The low `payload_bits` bits of the two's-complement value.
            let mask = (1i32 << payload_bits) - 1;
            assert_eq!(
                coded,
                i32::from(data) & mask,
                "payload for feature {feature}"
            );
        }
    }
}

#[test]
fn uleb_encode_multi_byte() {
    assert_eq!(uleb_encode(128), vec![0x80, 0x01]);
    assert_eq!(uleb_encode(256), vec![0x80, 0x02]);
}

#[test]
fn obu_header_basic() {
    let header = write_obu_header(ObuType::SequenceHeader, false);
    assert_eq!(header.len(), 1);
    assert_eq!(header[0], 0b0_0001_0_1_0);
}

#[test]
fn obu_header_frame() {
    let header = write_obu_header(ObuType::Frame, false);
    assert_eq!(header[0], 0b0_0110_0_1_0);
}

#[test]
fn temporal_delimiter_obu() {
    let td = write_temporal_delimiter();
    assert_eq!(td.len(), 2);
    assert_eq!(td[0], 0b0_0010_0_1_0);
    assert_eq!(td[1], 0);
}

#[test]
fn sequence_header_non_empty() {
    let sh = write_sequence_header(64, 64);
    assert!(sh.len() > 3, "sequence header should be > 3 bytes");
    assert_eq!(sh[0], 0b0_0001_0_1_0);
}

#[test]
fn still_frame_produces_valid_structure() {
    let tile_data = vec![0u8; 10];
    let bitstream = write_still_frame(64, 64, 128, &tile_data);
    assert!(bitstream.len() > 20, "bitstream should be substantial");
    assert_eq!(bitstream[0], 0b0_0010_0_1_0);
}

#[test]
fn bit_writer_basic() {
    let mut bw = BitWriter::new();
    bw.write_bits(0b1010, 4);
    bw.write_bits(0b1100, 4);
    assert_eq!(bw.bytes_written(), 1);
    assert_eq!(bw.data()[0], 0b10101100);
}

#[test]
fn bit_writer_cross_byte() {
    let mut bw = BitWriter::new();
    bw.write_bits(0xFF, 8);
    bw.write_bits(0x01, 1);
    assert_eq!(bw.bytes_written(), 2);
    assert_eq!(bw.data()[0], 0xFF);
    assert_eq!(bw.data()[1], 0x80);
}

#[test]
fn tile_info_single_sb() {
    // 64x64 = 1 SB → uniform + no increments, log2=0 (regression baseline)
    let mut wb = BitWriter::new();
    write_tile_info(&mut wb, 64, 64, 0, 0, 0, 64);
    // Should be just 1 bit (uniform_tile_spacing_flag)
    assert_eq!(wb.bit_offset, 1);
}

#[test]
fn tile_info_four_sbs() {
    // 128x128 = 4 SBs, log2=0 requested → uniform + 1 col increment +
    // 1 row stop bit (regression baseline: task #86 must not change
    // this byte shape when tile_rows_log2 == 0).
    let mut wb = BitWriter::new();
    write_tile_info(&mut wb, 128, 128, 0, 0, 0, 64);
    // uniform_flag (1) + col_increment_stop (1) + row_increment_stop (1) = 3
    assert_eq!(wb.bit_offset, 3);
}

#[test]
fn tile_info_128x128_two_tile_rows() {
    // 128x128 = 2x2 SBs, tile_rows_log2=1 (the task #86 acceptance
    // config): TileRowsLog2 hits maxLog2TileRows(1) exactly, so the
    // rows sub-syntax is ONE "1" bit and NO trailing stop bit (the
    // spec's `while (TileRowsLog2 < maxLog2TileRows)` loop condition
    // itself terminates the syntax, unlike the col case which stops
    // early via an explicit "0").
    assert_eq!(resolve_tile_rows_log2(128, 128, 1), 1);
    let mut wb = BitWriter::new();
    write_tile_info(&mut wb, 128, 128, 1, 0, 0, 64);
    // uniform(1) + col_stop(1) + row_inc(1) + context_update_tile_id
    // (1 bit, value=(1<<1)-1=1) + tile_size_bytes_minus_1(2 bits, 0) = 6
    assert_eq!(wb.bit_offset, 6);
    // bit sequence MSB-first: 1,0,1, 1, 0,0 -> 0b1011_0000
    assert_eq!(wb.data(), &[0b1011_0000]);
}

#[test]
fn tile_info_512x512_two_tile_rows() {
    // 512x512 = 8x8 SBs: maxLog2TileRows=3, so requesting log2=1 stops
    // BEFORE the max and needs the trailing "0" (unlike the 128x128
    // case above, which hit its max exactly and has no stop bit).
    assert_eq!(resolve_tile_rows_log2(512, 512, 1), 1);
    let mut wb = BitWriter::new();
    write_tile_info(&mut wb, 512, 512, 1, 0, 0, 64);
    // uniform(1) + col: maxLog2TileCols=3>0 -> stop(1) + row: one
    // "1" increment + one "0" stop (2 bits, since 1 < 3) +
    // context_update_tile_id(1 bit, value=1) + tile_size_bytes(2) = 7
    assert_eq!(wb.bit_offset, 7);
    // 1,0, 1,0, 1, 0,0 -> 0b1010_100(0 unwritten) = 0xA8
    assert_eq!(wb.data(), &[0b1010_1000]);
}

#[test]
fn tile_grid_actual_count_is_below_pow2_when_sb_count_does_not_divide() {
    // THE task-#96 defect, in one assertion. 512x384 at SB64 = 8x6 SBs.
    // C's algorithm (svt_aom_set_tile_info's comment block +
    // svt_av1_calculate_tile_rows): size_sb = ceil(6 / 2^2) = 2, then
    // fill 2-SB tiles until the picture ends -> start_sb 0,2,4 -> THREE
    // tiles, not four. `1 << log2` (= 4) is the REQUEST, never the count.
    let g = TileGrid::resolve(512, 384, 64, 2, 0);
    assert_eq!(g.tile_rows_log2, 2, "log2 is honoured");
    assert_eq!(g.tile_height_sb, 2);
    assert_eq!(g.tile_rows, 3, "ACTUAL rows, not 1<<2");
    assert_eq!(g.num_tiles(), 3);
    // Spans are contiguous, non-empty and cover the frame exactly —
    // the pre-fix `1 << log2` count produced a 4th EMPTY span [6,6).
    assert_eq!(g.row_span(0), (0, 2));
    assert_eq!(g.row_span(1), (2, 4));
    assert_eq!(g.row_span(2), (4, 6));

    // log2=3 -> size 1 -> exactly 6 tiles (8 requested).
    let g3 = TileGrid::resolve(512, 384, 64, 3, 0);
    assert_eq!((g3.tile_height_sb, g3.tile_rows), (1, 6));

    // The divisible case is unchanged: 256x256 = 4x4 SBs, log2=2 -> 4.
    let g4 = TileGrid::resolve(256, 256, 64, 2, 0);
    assert_eq!((g4.tile_height_sb, g4.tile_rows), (1, 4));
}

#[test]
fn tile_info_writes_actual_tile_count_in_context_update_tile_id() {
    // Regression witness for the CORRUPTION this fixed. 512x384,
    // rows_log2=2 -> 3 real tiles, so context_update_tile_id must be
    // 2 (NumTiles-1) in 2 bits. Writing the all-ones `(1<<log2)-1 = 3`
    // exceeds NumTiles-1 and aomdec rejects the frame with
    // "Invalid context_update_tile".
    let g = TileGrid::resolve(512, 384, 64, 2, 0);
    assert_eq!(g.num_tiles(), 3);
    let mut wb = BitWriter::new();
    write_tile_info(&mut wb, 512, 384, 2, 0, 0, 64);
    // uniform(1) + col stop(1: maxLog2TileCols=3>0) + rows: 2 "1"
    // increments then a "0" stop (2 < maxLog2TileRows=3) = 3 bits +
    // context_update_tile_id(2 bits) + tile_size_bytes_minus_1(2) = 9
    assert_eq!(wb.bit_offset, 9);
    // 1, 0, 1,1,0, 1,0, 0,0 -> 0b1011_0100 0b0xxx_xxxx
    assert_eq!(wb.data()[0], 0b1011_0100);
    assert_eq!(
        wb.data()[1] & 0b1000_0000,
        0,
        "ctx id LSB = 0 (value 2, not 3)"
    );
}

#[test]
fn tile_grid_resolves_columns_like_c_order() {
    // C resolves COLUMNS first, then recomputes minLog2TileRows
    // against the chosen columns (svt_aom_set_tile_info:2555-2578 +
    // write_tile_info_max_tile:2417). 512x384 = 8x6 SBs.
    let g = TileGrid::resolve(512, 384, 64, 0, 2);
    assert_eq!(g.tile_cols_log2, 2);
    assert_eq!((g.tile_width_sb, g.tile_cols), (2, 4));
    assert_eq!(g.col_span(0), (0, 2));
    assert_eq!(g.col_span(3), (6, 8));
    assert_eq!(g.num_tiles(), 4);

    // Columns clamp to maxLog2TileCols = tile_log2(min(sb_cols, 64)).
    // 256 wide at SB64 = 4 SB cols -> max log2 = 2, so 3 clamps to 2.
    assert_eq!(TileGrid::resolve(256, 256, 64, 0, 3).tile_cols_log2, 2);
    // A 1-SB-wide frame supports no column split at all.
    assert_eq!(TileGrid::resolve(64, 512, 64, 0, 4).tile_cols_log2, 0);

    // Rows x cols compose: 3 row tiles x 4 col tiles = 12.
    let g2 = TileGrid::resolve(512, 384, 64, 2, 2);
    assert_eq!((g2.tile_rows, g2.tile_cols, g2.num_tiles()), (3, 4, 12));
}

#[test]
fn resolve_tile_rows_log2_clamps_like_c() {
    // 64x64 = 1 SB row: maxLog2TileRows = tile_log2(1) = 0, so any
    // request clamps down to 0 (single tile row) — matches C's
    // svt_aom_set_tile_info AOMMIN(log2_tile_rows, max_log2_tile_rows).
    assert_eq!(resolve_tile_rows_log2(64, 64, 0), 0);
    assert_eq!(resolve_tile_rows_log2(64, 64, 1), 0);
    assert_eq!(resolve_tile_rows_log2(64, 64, 6), 0);
    // 128x128 = 2 SB rows: max = 1.
    assert_eq!(resolve_tile_rows_log2(128, 128, 0), 0);
    assert_eq!(resolve_tile_rows_log2(128, 128, 1), 1);
    assert_eq!(resolve_tile_rows_log2(128, 128, 5), 1);
    // 512x512 = 8 SB rows: max = 3.
    assert_eq!(resolve_tile_rows_log2(512, 512, 0), 0);
    assert_eq!(resolve_tile_rows_log2(512, 512, 1), 1);
    assert_eq!(resolve_tile_rows_log2(512, 512, 3), 3);
    assert_eq!(resolve_tile_rows_log2(512, 512, 10), 3);
}

#[test]
fn tile_size_bytes_minus_1_for_thresholds() {
    assert_eq!(tile_size_bytes_minus_1_for(&[]), 0);
    assert_eq!(tile_size_bytes_minus_1_for(&[0]), 0);
    assert_eq!(tile_size_bytes_minus_1_for(&[255]), 0);
    assert_eq!(tile_size_bytes_minus_1_for(&[256]), 1);
    assert_eq!(tile_size_bytes_minus_1_for(&[65535]), 1);
    assert_eq!(tile_size_bytes_minus_1_for(&[65536]), 2);
    assert_eq!(tile_size_bytes_minus_1_for(&[16_777_215]), 2);
    assert_eq!(tile_size_bytes_minus_1_for(&[16_777_216]), 3);
    // Only the non-last tiles matter — the max is taken over the slice
    // the caller passes (callers pass `lens[..len-1]`).
    assert_eq!(tile_size_bytes_minus_1_for(&[10, 65536, 20]), 2);
}

#[test]
fn build_tile_group_multi_variable_width_prefix() {
    // First tile > 255 bytes forces a 2-byte tile_size_minus_1 prefix
    // (matching C mem_put_varsize / the trailer this function's
    // caller must write identically into tile_info()).
    let tile0 = vec![0xABu8; 300];
    let tile1 = vec![0xCDu8; 10];
    let tsb1 = tile_size_bytes_minus_1_for(&[tile0.len()]);
    assert_eq!(tsb1, 1); // 300 >> 8 != 0, >> 16 == 0
    let out = build_tile_group_multi(&[tile0.clone(), tile1.clone()], tsb1);
    // header: tile_start_and_end_present_flag(0) + byte_align -> 1 byte of 0x00
    assert_eq!(out[0], 0x00);
    // 2-byte LE size_minus_1 = 299
    assert_eq!(&out[1..3], &299u16.to_le_bytes());
    assert_eq!(&out[3..3 + 300], tile0.as_slice());
    assert_eq!(&out[3 + 300..], tile1.as_slice());
}

#[test]
fn build_tile_group_multi_single_tile_delegates() {
    // len <= 1 must be byte-identical to build_tile_group_single
    // (no header bits at all) regardless of tile_size_bytes_minus_1.
    let tile0 = vec![1u8, 2, 3];
    assert_eq!(
        build_tile_group_multi(std::slice::from_ref(&tile0), 3),
        build_tile_group_single(&tile0)
    );
}

#[test]
fn tile_log2_values() {
    assert_eq!(tile_log2(0), 0);
    assert_eq!(tile_log2(1), 0);
    assert_eq!(tile_log2(2), 1);
    assert_eq!(tile_log2(3), 2);
    assert_eq!(tile_log2(4), 2);
    assert_eq!(tile_log2(5), 3);
}

#[test]
fn tile_log2_blk_matches_c_loop() {
    // C: for (k = 0; (blk_size << k) < target; k++) {}
    assert_eq!(tile_log2_blk(1, 0), 0);
    assert_eq!(tile_log2_blk(1, 1), 0);
    assert_eq!(tile_log2_blk(64, 2), 0); // 64 already >= 2
    assert_eq!(tile_log2_blk(2304, 4), 0); // MAX_TILE_AREA_SB-scale, tiny target
    assert_eq!(tile_log2_blk(1, 5), 3); // matches tile_log2(5) == 3
    // Rejection must terminate even when the next power exceeds u32.
    assert_eq!(tile_log2_blk(2304, u32::MAX), 21);
    for sb_size in [64, 128] {
        assert!(
            TileLimits::for_frame(4096, u32::MAX, sb_size)
                .untileable_reason(0)
                .is_some()
        );
    }
}

/// Mono SH/FH byte goldens. Originally captured before the 4:2:0 work
/// (2026-07-13 @ 264deaf03) to pin the mono path while chroma landed;
/// re-captured when CDEF signaling landed: the SH gained the
/// enable_cdef=1 bit (0x06 -> 0x26 in byte 6 of the reduced SH,
/// 0xc3 -> 0xd3 in the full SH) and every FH gained the 10-bit
/// zero-strength cdef_params tail (FH grows 5 -> 6 bytes).
/// Re-captured 2026-07-13 when SH C-parity landed (IDENTITY-STATUS item
/// S1-S3): seq_level_idx now auto-derives via the C
/// set_bitstream_level_tier port (64x64@30 → level 2.0 → idx 0, was
/// pinned 8/4.0 — reduced SH byte 0 0x1a → 0x18) and color_range now
/// honors ColorDescription::full_range (srgb() is studio range →
/// bit 0, was hardcoded 1 — byte 7 0x03 → 0x02). The full SH
/// additionally LOSES its seq_tier bit (only coded for level idx > 7,
/// spec 5.5.1 / C entropy_coding.c:3790), shifting every later field
/// one bit left. All bytes hand-verified field-by-field against spec
/// 5.5.1/5.5.2.
#[test]
fn mono_headers_unchanged_golden() {
    assert_eq!(
        write_sequence_header(64, 64),
        [
            0x0a, 0x09, 0x18, 0x15, 0x7f, 0xfc, 0x26, 0x02, 0x1a, 0x02, 0x40
        ]
    );
    // FH golden re-captured when TX_MODE_SELECT landed (bit 42
    // tx_mode_select 0->1: byte 5 0x00 -> 0x20, hand-verified).
    assert_eq!(
        write_key_frame_header(64, 64, 30),
        [0x11, 0xe0, 0x00, 0x00, 0x00, 0x20]
    );
    // CHANGED 2026-08-31 (inter campaign C1a) — the FULL (video-mode)
    // header only. The reduced/still goldens above are untouched.
    //
    // The previous bytes encoded a header C never emits: they omitted the
    // five `initial_display_delay` bits (C sets the flag for every
    // non-reduced header, enc_handle.c:4990) and placed
    // `order_hint_bits_minus_1` BEFORE the seq_choose_* bits instead of
    // after (entropy_coding.c:2836). Those are two of the four defects the
    // inter refusal in pipeline.rs names, so the old golden was pinning
    // the bug.
    //
    // The new bytes decode field-by-field (tools/sh_fields.py) as:
    //   profile 0, still 0, reduced 0, timing 0,
    //   initial_display_delay_present 1, op_cnt_minus_1 0, op_idc 0,
    //   seq_level_idx 0, idd_present_for_op 1,
    //   initial_display_delay_minus_1 0  (hierarchical_levels 0 -> delay 1),
    //   frame_{width,height}_bits_minus_1 5/5, max dims 63/63,
    //   frame_id_numbers_present 0, use_128x128 0, and every tool bit 0
    //   except enable_order_hint 1 (SeqTools::default), then
    //   seq_choose_screen_content_tools 1, seq_choose_integer_mv 1,
    //   order_hint_bits_minus_1 6, superres 0, cdef 1, restoration 0,
    //   high_bitdepth 0, mono_chrome 1, color_description_present 1.
    //
    // Stronger corroboration than this hand-golden: with the same fixes
    // the PIPELINE's video-mode sequence header is byte-identical to the
    // real C encoder's (13 bytes, 0a0b02000004157ffc6af98040) on the
    // gradient 64x64 q40 p6 cell.
    assert_eq!(
        write_sequence_header_full(64, 64),
        [
            0x0a, 0x0d, 0x02, 0x00, 0x00, 0x04, 0x15, 0x7f, 0xfc, 0x02, 0x79, 0x30, 0x10, 0xd0,
            0x12
        ]
    );
}

/// The pipeline-default SH must be byte-identical to what C SVT-AV1
/// emits at the identity-harness matched config (uniform/gradient
/// 64x64 still, preset 13, defaults: CICP unspecified 2/2/2 →
/// color_description_present_flag=0, studio range, 30 fps → level
/// 2.0). C golden = the SEQUENCE_HEADER OBU payload captured by
/// tools/identity_diff.sh from libSvtAv1Enc v4.2.0-rc (see
/// docs/IDENTITY-STATUS.md "uniform 64x64 q40 p13").
#[test]
fn sh_420_default_byte_identical_to_c() {
    const C_SH_PAYLOAD: [u8; 6] = [0x18, 0x15, 0x7f, 0xfc, 0x20, 0x08];
    let ours = write_sequence_header_ex(
        64,
        64,
        true,
        8,
        &ColorDescription::default(),
        false,
        30.0,
        SeqTools::default(),
    );
    assert_eq!(ours[0], 0b0_0001_0_1_0); // SH OBU header
    assert_eq!(ours[1] as usize, C_SH_PAYLOAD.len()); // leb128 size
    assert_eq!(&ours[2..], &C_SH_PAYLOAD, "SH payload != C bytes");
}

/// The preset<=6 allintra SH must be byte-identical to what C SVT-AV1
/// emits at the identity-harness matched config with the M6 tool bits
/// on. C golden = the SEQUENCE_HEADER OBU payload captured by
/// tools/identity_diff.sh from libSvtAv1Enc v4.2.0-rc at uniform
/// 64x64 q40 preset 6 (docs/IDENTITY-STATUS.md "uniform 64x64 q40
/// p6": `[18] 15 7f fd 30 08`). vs the p13 payload the only changes
/// are bit 31 (enable_filter_intra 0->1: byte 3 0xfc->0xfd) and bit
/// 35 (enable_restoration 0->1: byte 4 0x20->0x30) — hand-verified
/// against the differ's field walk (@31 / @35).
#[test]
fn sh_420_p6_tools_byte_identical_to_c() {
    const C_SH_PAYLOAD_P6: [u8; 6] = [0x18, 0x15, 0x7f, 0xfd, 0x30, 0x08];
    let ours = write_sequence_header_ex(
        64,
        64,
        true,
        8,
        &ColorDescription::default(),
        false,
        30.0,
        SeqTools {
            separate_uv_delta_q: false,
            film_grain_params_present: false,
            enable_filter_intra: true,
            enable_intra_edge_filter: false,
            enable_restoration: true,
            use_128x128_superblock: false,
            enable_superres: false,
            chroma_sample_position: 0,
            // This is a STILL (reduced) header: none of the inter tool
            // bits nor the display-delay fields are written, so the
            // defaults are inert here by construction.
            ..SeqTools::default()
        },
    );
    assert_eq!(ours[0], 0b0_0001_0_1_0); // SH OBU header
    assert_eq!(ours[1] as usize, C_SH_PAYLOAD_P6.len()); // leb128 size
    assert_eq!(&ours[2..], &C_SH_PAYLOAD_P6, "p6 SH payload != C bytes");
}

/// lr_params() (spec 5.9.20) with every plane RESTORE_NONE adds
/// exactly NumPlanes x 2 zero bits after cdef_params and codes no
/// unit-size fields (C encode_restoration_mode skips the !all_none /
/// !chroma_none blocks, entropy_coding.c:2284-2306). At the identity
/// config C's p6 FH decodes to 70 bits where p13 is 64 — the +6 is
/// the three lr_type pairs (IDENTITY-STATUS "uniform 64x64 q40 p6").
#[test]
fn fh_lr_params_all_none_bit_shape() {
    // 4:2:0: 3 planes -> +6 bits.
    let base = key_frame_header_bits(64, 64, 160, true, false, [19, 19, 9, 9], [4, 14, 14], false);
    let lr = key_frame_header_bits(64, 64, 160, true, false, [19, 19, 9, 9], [4, 14, 14], true);
    assert_eq!(base.bit_offset, 64, "p13-shape FH must stay 64 bits");
    assert_eq!(lr.bit_offset, 70, "3-plane all-NONE lr_params adds 6 bits");
    // Mono: 1 plane -> +2 bits.
    let base_m = key_frame_header_bits(64, 64, 160, true, true, [19, 19, 0, 0], [4, 14, 0], false);
    let lr_m = key_frame_header_bits(64, 64, 160, true, true, [19, 19, 0, 0], [4, 14, 0], true);
    assert_eq!(lr_m.bit_offset - base_m.bit_offset, 2);
    // The inserted lr_type bits are zeros (RESTORE_NONE), positioned
    // between cdef_params and tx_mode: everything before is equal,
    // and the 2 trailing fields (tx_mode_select=1, reduced_tx_set=0)
    // follow the inserted zeros.
    let b = lr.into_data();
    let base_b = base.into_data();
    assert_eq!(b[..7], base_b[..7], "bits before lr_params must match");
}

/// The level ladder port must reproduce C's set_bitstream_level_tier
/// picks across the rungs (and the {9,3}→31 fallthrough).
#[test]
fn seq_level_ladder_matches_c() {
    assert_eq!(compute_seq_level_idx(64, 64, 30.0), 0); // 2.0
    assert_eq!(compute_seq_level_idx(512, 288, 30.0), 0); // 2.0 edge
    assert_eq!(compute_seq_level_idx(704, 396, 30.0), 1); // 2.1
    assert_eq!(compute_seq_level_idx(1088, 612, 30.0), 4); // 3.0
    assert_eq!(compute_seq_level_idx(1376, 774, 30.0), 5); // 3.1
    assert_eq!(compute_seq_level_idx(1920, 1080, 30.0), 8); // 4.0
    assert_eq!(compute_seq_level_idx(1920, 1080, 60.0), 9); // 4.1
    assert_eq!(compute_seq_level_idx(3840, 2160, 30.0), 12); // 5.0
    assert_eq!(compute_seq_level_idx(3840, 2160, 60.0), 13); // 5.1
    assert_eq!(compute_seq_level_idx(3840, 2160, 120.0), 14); // 5.2
    assert_eq!(compute_seq_level_idx(7680, 4320, 30.0), 16); // 6.0
    assert_eq!(compute_seq_level_idx(7680, 4320, 60.0), 17); // 6.1
    assert_eq!(compute_seq_level_idx(7680, 4320, 120.0), 18); // 6.2
    // Nothing matches → C default bl {9,3} → ((9-2)<<2)+3 = 31.
    assert_eq!(compute_seq_level_idx(16384, 8704, 30.0), 31);
    // dim_mult check: 4096 wide fits level 2.0 pels? No — width
    // 4096 > 512*4 = 2048, and pels too big; lands on 5.0 via pels.
    assert_eq!(compute_seq_level_idx(4096, 2176, 30.0), 12);
}

/// Pin the 4:2:0 (mono_chrome=0) sequence-header bit layout for a
/// 64x64 reduced (still-picture) SH, hand-derived field by field from
/// AV1 spec 5.5.1/5.5.2 and cross-checked against libaom's
/// av1_read_color_config (decodeframe.c:4167).
#[test]
fn sh_420_bit_layout_pinned() {
    // Hand-assemble the expected payload (independent field spelling —
    // any field-order/width regression in the writer breaks equality).
    let mut wb = BitWriter::new();
    wb.write_bits(0, 3); // seq_profile = 0 (Main: 8/10-bit 4:2:0)
    wb.write_bit(true); // still_picture = 1
    wb.write_bit(true); // reduced_still_picture_header = 1
    wb.write_bits(0, 5); // seq_level_idx = 0 (auto: 64x64@30 → 2.0)
    wb.write_bits(5, 4); // frame_width_bits_minus_1 (64 -> 6 bits)
    wb.write_bits(5, 4); // frame_height_bits_minus_1
    wb.write_bits(63, 6); // max_frame_width_minus_1
    wb.write_bits(63, 6); // max_frame_height_minus_1
    wb.write_bit(false); // use_128x128_superblock = 0
    wb.write_bit(false); // enable_filter_intra = 0
    wb.write_bit(false); // enable_intra_edge_filter = 0
    wb.write_bit(false); // enable_superres = 0
    wb.write_bit(true); // enable_cdef = 1 (CDEF signaling landed)
    wb.write_bit(false); // enable_restoration = 0
    // color_config() per spec 5.5.2, mono_chrome = 0 branch:
    wb.write_bit(false); // high_bitdepth = 0 (8-bit)
    wb.write_bit(false); // mono_chrome = 0 -> NumPlanes = 3
    wb.write_bit(true); // color_description_present_flag = 1
    wb.write_bits(1, 8); // color_primaries = CP_BT_709
    wb.write_bits(13, 8); // transfer_characteristics = TC_SRGB
    wb.write_bits(1, 8); // matrix_coefficients = MC_BT_709
    // (cp,tc,mc) != (BT709, SRGB, IDENTITY) -> decoder reads:
    wb.write_bit(false); // color_range = 0 (srgb() is studio range)
    // seq_profile == 0 -> subsampling_x = subsampling_y = 1, NO bits.
    // subsampling_x && subsampling_y -> chroma_sample_position f(2):
    wb.write_bits(0, 2); // chroma_sample_position = CSP_UNKNOWN
    wb.write_bit(false); // separate_uv_delta_q = 0
    wb.write_bit(false); // film_grain_params_present = 0
    wb.write_bit(true); // trailing_one_bit
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bits(0, 8 - remainder); // trailing zero pad
    }
    let expected_payload = wb.into_data();
    let expected_obu = write_obu(ObuType::SequenceHeader, &expected_payload);

    let got = write_sequence_header_ex(
        64,
        64,
        true,
        8,
        &ColorDescription::srgb(),
        false,
        30.0,
        SeqTools::default(),
    );
    assert_eq!(
        got, expected_obu,
        "420 SH layout drifted from spec derivation"
    );
}

/// The 4:2:0 SH's color_config must be BIT-IDENTICAL to what C SVT-AV1
/// writes for the matching CICP config. C golden captured from
/// v4.2.0-rc:
///
/// ```text
/// SvtAv1EncApp -i grad64.y4m -b c_420.ivf --avif 1 -q 30 --preset 4 \
///   --color-primaries 1 --transfer-characteristics 13 \
///   --matrix-coefficients 1 --color-range 1 -n 1
/// ```
///
/// (64x64 4:2:0 still picture -> reduced SH, payload 9 bytes.) This
/// golden predates the level auto-derivation and the preset tool
/// bits, so full SH byte-equality is not asserted here; the
/// preamble/tool fields are all fixed-width, so both color_configs
/// start at bit 36 and must match field-for-field — asserted below
/// by parsing both. (Whole-SH byte goldens vs C live in
/// sh_420_default_byte_identical_to_c (p13, tools off) and
/// sh_420_p6_tools_byte_identical_to_c (p6, tools on).)
#[test]
fn sh_420_color_config_matches_c_reference() {
    const C_SH_PAYLOAD: [u8; 9] = [0x18, 0x15, 0x7f, 0xfd, 0x32, 0x02, 0x1a, 0x03, 0x08];

    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
    }
    impl Bits<'_> {
        fn f(&mut self, n: usize) -> u32 {
            let mut v = 0;
            for _ in 0..n {
                let byte = self.d[self.pos / 8];
                let bit = (byte >> (7 - (self.pos % 8))) & 1;
                v = (v << 1) | u32::from(bit);
                self.pos += 1;
            }
            v
        }
    }

    // Reduced-SH preamble is fixed-width: profile(3) still(1) reduced(1)
    // level(5) wbits(4) hbits(4) w(6) h(6) sb128(1) filter_intra(1)
    // intra_edge_filter(1) superres(1) cdef(1) restoration(1) = 36 bits.
    fn parse(payload: &[u8]) -> (u32, u32, [u32; 10]) {
        let mut b = Bits { d: payload, pos: 0 };
        assert_eq!(b.f(3), 0, "seq_profile 0");
        assert_eq!(b.f(1), 1, "still_picture");
        assert_eq!(b.f(1), 1, "reduced_still_picture_header");
        let level = b.f(5);
        assert_eq!(b.f(4), 5); // frame_width_bits_minus_1
        assert_eq!(b.f(4), 5); // frame_height_bits_minus_1
        assert_eq!(b.f(6), 63); // max_frame_width_minus_1
        assert_eq!(b.f(6), 63); // max_frame_height_minus_1
        assert_eq!(b.f(1), 0, "use_128x128_superblock");
        let tools = (b.f(1) << 4) | (b.f(1) << 3) | (b.f(1) << 2) | (b.f(1) << 1) | b.f(1);
        // color_config from here:
        let cc = [
            b.f(1), // high_bitdepth
            b.f(1), // mono_chrome
            b.f(1), // color_description_present_flag
            b.f(8), // color_primaries
            b.f(8), // transfer_characteristics
            b.f(8), // matrix_coefficients
            b.f(1), // color_range
            b.f(2), // chroma_sample_position (profile 0, 420)
            b.f(1), // separate_uv_delta_q
            b.f(1), // film_grain_params_present
        ];
        assert_eq!(b.f(1), 1, "trailing_one_bit");
        (level, tools, cc)
    }

    // The C capture passed --color-range 1 (full), so the matching Rust
    // config is sRGB CICP + full range (the writer now honors
    // full_range instead of hardcoding 1).
    let color = ColorDescription {
        full_range: true,
        ..ColorDescription::srgb()
    };
    let ours = write_sequence_header_ex(64, 64, true, 8, &color, false, 30.0, SeqTools::default());
    // Strip OBU header (1 byte) + leb128 size (1 byte for these sizes).
    assert_eq!(ours[0], 0b0_0001_0_1_0);
    let our_payload = &ours[2..];

    let (c_level, c_tools, c_cc) = parse(&C_SH_PAYLOAD);
    let (r_level, r_tools, r_cc) = parse(our_payload);

    assert_eq!(c_cc, r_cc, "color_config fields must match C bit-for-bit");
    assert_eq!(c_cc[1], 0, "mono_chrome = 0");
    assert_eq!(&c_cc[3..6], &[1, 13, 1], "CICP cp/tc/mc");
    assert_eq!(c_cc[6], 1, "color_range full");
    assert_eq!(c_cc[7], 0, "chroma_sample_position CSP_UNKNOWN");
    assert_eq!(c_cc[8], 0, "separate_uv_delta_q");

    // Level now auto-derives exactly like C (set_bitstream_level_tier
    // port): 64x64@30 → level 2.0 → idx 0 on both sides.
    assert_eq!(c_level, 0, "C derives level 2.0 for 64x64");
    assert_eq!(r_level, 0, "we derive level 2.0 too (C-exact port)");
    // Tool bits (bit 4=filter_intra .. bit 0=restoration; both sides
    // have intra_edge_filter=0, superres=0): C enables
    // filter_intra/cdef/restoration. Our cdef bit now MATCHES C (the
    // port landed); filter_intra/restoration stay 0 until theirs land.
    assert_eq!(c_tools, 0b10011);
    assert_eq!(r_tools, 0b00010, "enable_cdef=1, rest pending ports");
}

/// Pin the 4:2:0 (NumPlanes=3) key-frame-header layout for a reduced SH
/// at 64x64 q30 — hand-derived from spec 5.9.2/5.9.12 and libaom
/// setup_quantization (decodeframe.c:1818): with separate_uv_delta_q=0
/// the decoder reads NO diff_uv_delta, then DeltaQUDc + DeltaQUAc
/// (delta_coded=0 each), and V reuses U (no V bits).
#[test]
fn fh_420_bit_layout_pinned() {
    let mut wb = BitWriter::new();
    wb.write_bit(false); // disable_cdf_update = 0
    wb.write_bit(false); // allow_screen_content_tools = 0
    wb.write_bit(false); // render_and_frame_size_different = 0
    wb.write_bit(true); // tile_info: uniform_tile_spacing_flag (64x64 = 1 SB)
    wb.write_bits(30, 8); // base_q_idx = 30
    wb.write_bit(false); // DeltaQYDc: delta_coded = 0
    wb.write_bit(false); // DeltaQUDc: delta_coded = 0 (NumPlanes=3)
    wb.write_bit(false); // DeltaQUAc: delta_coded = 0 (V reuses U)
    wb.write_bit(false); // using_qmatrix = 0
    wb.write_bit(false); // segmentation_enabled = 0
    wb.write_bit(false); // delta_q_present = 0 (base_q_idx > 0)
    wb.write_bits(0, 6); // loop_filter_level[0] = 0
    wb.write_bits(0, 6); // loop_filter_level[1] = 0
    // levels [2]/[3] not coded: (level[0] || level[1]) == 0
    wb.write_bits(0, 3); // loop_filter_sharpness = 0
    wb.write_bit(false); // loop_filter_delta_enabled = 0
    wb.write_bits(0, 2); // cdef_damping_minus_3 (damping 3)
    wb.write_bits(0, 2); // cdef_bits = 0
    wb.write_bits(0, 6); // cdef_y_strength[0]
    wb.write_bits(0, 6); // cdef_uv_strength[0] (NumPlanes=3)
    wb.write_bit(true); // tx_mode_select = 1 (TX_MODE_SELECT, like C)
    wb.write_bit(false); // reduced_tx_set = 0
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bits(0, 8 - remainder); // byte_alignment(): zero bits
    }
    let expected = wb.into_data();

    let got = write_key_frame_header_full(64, 64, 30, true, false, [0; 4], [3, 0, 0], false);
    assert_eq!(got, expected, "420 FH layout drifted from spec derivation");

    // The 420 FH is exactly the mono FH with two zero bits
    // (DeltaQUDc/DeltaQUAc delta_coded=0) plus the 6-bit
    // cdef_uv_strength inserted. Pin the pre-alignment bit counts
    // (which set the decoder's field boundaries): mono 34+10 cdef
    // bits = 44, 420 36+16 = 52. Since TX_MODE_SELECT landed the
    // differing region is no longer all-zero: the tx_mode_select=1
    // bit sits at bit 42 (mono) vs bit 50 (420), so mono byte 5 is
    // 0x20 while 420 has byte 5 = 0x00 and the set bit in byte 6.
    let bits_420 =
        key_frame_header_bits(64, 64, 30, true, false, [0; 4], [3, 0, 0], false).bit_offset;
    let bits_mono =
        key_frame_header_bits(64, 64, 30, true, true, [0; 4], [3, 0, 0], false).bit_offset;
    assert_eq!(bits_mono, 44, "mono reduced-SH FH is 44 bits pre-align");
    assert_eq!(bits_420, 52, "420 adds DeltaQUDc + DeltaQUAc + uv cdef");
    let mono = write_key_frame_header_full(64, 64, 30, true, true, [0; 4], [3, 0, 0], false);
    assert_eq!(got.len(), 7);
    assert_eq!(mono.len(), 6);
    assert_eq!(&got[..5], &mono[..5], "shared prefix through cdef");
    assert_eq!(mono[5], 0x20, "mono: tx_mode_select at bit 42");
    assert_eq!(got[5], 0x00, "420: uv cdef strength bits still zero");
    assert_eq!(got[6], 0x20, "420: tx_mode_select at bit 50");
}

/// Pin loop_filter_params() with real levels (spec 5.9.11; C
/// encode_loopfilter, entropy_coding.c:2338): levels [2]/[3] are coded
/// only for NumPlanes=3 AND (level[0] || level[1]); sharpness and
/// delta_enabled=0 follow. Mono never codes chroma levels.
#[test]
fn fh_loop_filter_level_bit_layout() {
    // Mono: nonzero luma levels add no chroma bits.
    let zero = key_frame_header_bits(64, 64, 30, true, true, [0; 4], [3, 0, 0], false).bit_offset;
    let mono =
        key_frame_header_bits(64, 64, 30, true, true, [2, 2, 1, 1], [3, 0, 0], false).bit_offset;
    assert_eq!(mono, zero, "mono FH never codes chroma levels");

    // 420: +12 bits (two 6-bit chroma levels) when l0||l1.
    let z420 = key_frame_header_bits(64, 64, 30, true, false, [0; 4], [3, 0, 0], false).bit_offset;
    let c420 =
        key_frame_header_bits(64, 64, 30, true, false, [2, 2, 1, 1], [3, 0, 0], false).bit_offset;
    assert_eq!(c420, z420 + 12, "420 FH codes U/V levels iff l0||l1");
    // Zero luma levels suppress the chroma level fields even if
    // uv levels are nonzero (the decoder cannot read them).
    let z420uv =
        key_frame_header_bits(64, 64, 30, true, false, [0, 0, 1, 1], [3, 0, 0], false).bit_offset;
    assert_eq!(z420uv, z420);

    // Hand-derive the mono field bytes with levels [3,3,_,_]: identical
    // to the zero-level header except the two 6-bit level fields.
    let mut wb = BitWriter::new();
    wb.write_bit(false); // disable_cdf_update
    wb.write_bit(false); // allow_screen_content_tools
    wb.write_bit(false); // render_and_frame_size_different
    wb.write_bit(true); // tile_info: uniform_tile_spacing_flag
    wb.write_bits(30, 8); // base_q_idx
    wb.write_bit(false); // DeltaQYDc
    wb.write_bit(false); // using_qmatrix
    wb.write_bit(false); // segmentation_enabled
    wb.write_bit(false); // delta_q_present
    wb.write_bits(3, 6); // loop_filter_level[0]
    wb.write_bits(3, 6); // loop_filter_level[1]
    wb.write_bits(0, 3); // loop_filter_sharpness
    wb.write_bit(false); // loop_filter_delta_enabled
    wb.write_bits(0, 2); // cdef_damping_minus_3
    wb.write_bits(0, 2); // cdef_bits
    wb.write_bits(0, 6); // cdef_y_strength[0]
    wb.write_bit(true); // tx_mode_select = 1 (TX_MODE_SELECT, like C)
    wb.write_bit(false); // reduced_tx_set
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bits(0, 8 - remainder);
    }
    assert_eq!(
        write_key_frame_header_full(64, 64, 30, true, true, [3, 3, 0, 0], [3, 0, 0], false),
        wb.into_data(),
        "mono FH with levels drifted from spec derivation"
    );
}

/// Pin cdef_params() (spec 5.9.19; C encode_cdef entropy_coding.c:2398)
/// with real values: damping_minus_3 then cdef_bits=0 then one 6-bit
/// strength per coded plane type — uv only for NumPlanes=3 (libaom
/// setup_cdef reads uv iff num_planes > 1).
#[test]
fn fh_cdef_params_bit_layout() {
    let base = key_frame_header_bits(64, 64, 220, true, true, [0; 4], [3, 0, 0], false).bit_offset;
    // Strength/damping values change bits, never the field count.
    let hot = key_frame_header_bits(64, 64, 220, true, true, [0; 4], [6, 43, 7], false).bit_offset;
    assert_eq!(base, hot, "mono cdef fields are fixed-width");
    let b420 =
        key_frame_header_bits(64, 64, 220, true, false, [0; 4], [6, 43, 7], false).bit_offset;
    assert_eq!(
        b420,
        hot + 2 + 6,
        "420 adds chroma delta-q (2) + uv strength (6)"
    );

    // Hand-derive the mono FH at qindex 220 with damping 6, y=43:
    let mut wb = BitWriter::new();
    wb.write_bit(false); // disable_cdf_update
    wb.write_bit(false); // allow_screen_content_tools
    wb.write_bit(false); // render_and_frame_size_different
    wb.write_bit(true); // tile_info: uniform_tile_spacing_flag
    wb.write_bits(220, 8); // base_q_idx
    wb.write_bit(false); // DeltaQYDc
    wb.write_bit(false); // using_qmatrix
    wb.write_bit(false); // segmentation_enabled
    wb.write_bit(false); // delta_q_present
    wb.write_bits(0, 6); // loop_filter_level[0]
    wb.write_bits(0, 6); // loop_filter_level[1]
    wb.write_bits(0, 3); // loop_filter_sharpness
    wb.write_bit(false); // loop_filter_delta_enabled
    wb.write_bits(3, 2); // cdef_damping_minus_3 = 6 - 3
    wb.write_bits(0, 2); // cdef_bits = 0
    wb.write_bits(43, 6); // cdef_y_strength[0] = pri 10, sec 3
    wb.write_bit(true); // tx_mode_select = 1 (TX_MODE_SELECT, like C)
    wb.write_bit(false); // reduced_tx_set
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bits(0, 8 - remainder);
    }
    assert_eq!(
        write_key_frame_header_full(64, 64, 220, true, true, [0; 4], [6, 43, 0], false),
        wb.into_data(),
        "mono FH cdef_params drifted from spec derivation"
    );
}
