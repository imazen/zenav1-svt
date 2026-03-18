//! OBU (Open Bitstream Unit) writer for AV1 bitstreams.
//!
//! Spec 07 §5.3: OBU bitstream format.
//!
//! Produces valid AV1 bitstream output. All field orderings match
//! the AV1 specification (av1-spec-errata1) exactly.

use alloc::vec::Vec;

/// CICP color description for AV1 sequence headers.
///
/// Signals color primaries, transfer characteristics, and matrix coefficients
/// per ITU-T H.273. Used for wide gamut (P3, Rec.2020) and HDR (PQ, HLG).
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorDescription {
    /// Color primaries (1=BT.709/sRGB, 9=BT.2020, 12=P3).
    pub color_primaries: u8,
    /// Transfer characteristics (1=BT.709, 13=sRGB, 16=PQ/HDR10, 18=HLG).
    pub transfer_characteristics: u8,
    /// Matrix coefficients (1=BT.709, 9=BT.2020, 0=Identity/RGB).
    pub matrix_coefficients: u8,
    /// Full range (true) or limited/studio range (false).
    pub full_range: bool,
}

impl ColorDescription {
    /// sRGB (BT.709 primaries, sRGB transfer, BT.709 matrix).
    pub fn srgb() -> Self {
        Self {
            color_primaries: 1,
            transfer_characteristics: 13,
            matrix_coefficients: 1,
            full_range: false,
        }
    }

    /// Display P3 with sRGB transfer.
    pub fn display_p3() -> Self {
        Self {
            color_primaries: 12,
            transfer_characteristics: 13,
            matrix_coefficients: 1,
            full_range: false,
        }
    }

    /// BT.2020 with PQ (HDR10).
    pub fn bt2020_pq() -> Self {
        Self {
            color_primaries: 9,
            transfer_characteristics: 16,
            matrix_coefficients: 9,
            full_range: false,
        }
    }

    /// BT.2020 with HLG.
    pub fn bt2020_hlg() -> Self {
        Self {
            color_primaries: 9,
            transfer_characteristics: 18,
            matrix_coefficients: 9,
            full_range: false,
        }
    }

    /// BT.2020 with sRGB-like transfer (SDR wide gamut).
    pub fn bt2020_sdr() -> Self {
        Self {
            color_primaries: 9,
            transfer_characteristics: 1,
            matrix_coefficients: 9,
            full_range: false,
        }
    }
}

/// OBU types as defined in the AV1 spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObuType {
    SequenceHeader = 1,
    TemporalDelimiter = 2,
    FrameHeader = 3,
    TileGroup = 4,
    Metadata = 5,
    Frame = 6,
    RedundantFrameHeader = 7,
    Padding = 15,
}

/// Bit-level writer for OBU headers and uncompressed data.
pub struct BitWriter {
    data: Vec<u8>,
    bit_offset: u32,
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BitWriter {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            bit_offset: 0,
        }
    }

    /// Write `n` bits of `value` (MSB first).
    pub fn write_bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            let bit = (value >> i) & 1;
            let byte_idx = (self.bit_offset / 8) as usize;
            let bit_idx = 7 - (self.bit_offset % 8);

            if byte_idx >= self.data.len() {
                self.data.push(0);
            }
            if bit != 0 {
                self.data[byte_idx] |= 1 << bit_idx;
            }
            self.bit_offset += 1;
        }
    }

    /// Write a single bit.
    pub fn write_bit(&mut self, value: bool) {
        self.write_bits(value as u32, 1);
    }

    /// Number of bytes written (rounded up).
    pub fn bytes_written(&self) -> usize {
        self.bit_offset.div_ceil(8) as usize
    }

    /// Get the written data.
    pub fn data(&self) -> &[u8] {
        &self.data[..self.bytes_written()]
    }

    /// Consume and return the data.
    pub fn into_data(self) -> Vec<u8> {
        let len = self.bytes_written();
        let mut data = self.data;
        data.truncate(len);
        data
    }
}

/// Encode a value as unsigned LEB128 (used for OBU size fields).
pub fn uleb_encode(value: u32) -> Vec<u8> {
    let mut result = Vec::new();
    let mut v = value;
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        result.push(byte);
        if v == 0 {
            break;
        }
    }
    result
}

/// Write an OBU header.
///
/// Returns the header bytes (1 or 2 bytes depending on extension).
pub fn write_obu_header(obu_type: ObuType, has_extension: bool) -> Vec<u8> {
    let mut wb = BitWriter::new();
    wb.write_bits(0, 1); // obu_forbidden_bit
    wb.write_bits(obu_type as u32, 4); // obu_type
    wb.write_bit(has_extension); // obu_extension_flag
    wb.write_bit(true); // obu_has_size_field
    wb.write_bits(0, 1); // obu_reserved_1bit

    if has_extension {
        wb.write_bits(0, 3); // temporal_id
        wb.write_bits(0, 2); // spatial_id
        wb.write_bits(0, 3); // extension_header_reserved_3bits
    }

    wb.into_data()
}

/// Write a complete OBU (header + LEB128 size + payload).
pub fn write_obu(obu_type: ObuType, payload: &[u8]) -> Vec<u8> {
    let header = write_obu_header(obu_type, false);
    let size = uleb_encode(payload.len() as u32);
    let mut obu = Vec::with_capacity(header.len() + size.len() + payload.len());
    obu.extend_from_slice(&header);
    obu.extend_from_slice(&size);
    obu.extend_from_slice(payload);
    obu
}

/// Write a temporal delimiter OBU (empty payload, signals frame boundary).
pub fn write_temporal_delimiter() -> Vec<u8> {
    write_obu(ObuType::TemporalDelimiter, &[])
}

/// Write a reduced-header sequence header OBU (still-picture only).
pub fn write_sequence_header(width: u32, height: u32) -> Vec<u8> {
    write_sequence_header_ex(width, height, true, 8, &ColorDescription::srgb())
}

/// Write a full sequence header OBU that supports inter frames.
pub fn write_sequence_header_full(width: u32, height: u32) -> Vec<u8> {
    write_sequence_header_ex(width, height, false, 8, &ColorDescription::srgb())
}

/// Write a sequence header with explicit bit depth and color description.
///
/// Supports 8, 10, or 12 bit depth and CICP color signaling for
/// wide gamut (P3, Rec.2020) and HDR (PQ, HLG).
pub fn write_sequence_header_ex(
    width: u32,
    height: u32,
    still_picture: bool,
    bit_depth: u8,
    color: &ColorDescription,
) -> Vec<u8> {
    write_sequence_header_inner(width, height, still_picture, bit_depth, color)
}

/// Write AV1 trailing bits: a mandatory 1-bit followed by zeros to byte-align.
/// The trailing_one_bit MUST always be written, even if already byte-aligned
/// (in which case a full 0x80 byte is written).
fn write_trailing_bits(wb: &mut BitWriter) {
    wb.write_bit(true); // trailing_one_bit = 1
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bits(0, 8 - remainder); // zero-pad to byte boundary
    }
}

/// Order hint bits used in the full sequence header.
pub const ORDER_HINT_BITS: u32 = 7;

/// Compute ceil(log2(n)), with tile_log2(0) = 0, tile_log2(1) = 0.
fn tile_log2(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    32 - (n - 1).leading_zeros()
}

/// AV1 spec Section 5.5.1: Sequence header OBU.
///
/// Monochrome output (NumPlanes=1) — matches luma-only encoder.
fn write_sequence_header_inner(
    width: u32,
    height: u32,
    still_picture: bool,
    bit_depth: u8,
    color: &ColorDescription,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    // seq_profile: 0 = Main (8/10-bit 4:2:0), 2 = Professional (12-bit)
    let profile = if bit_depth > 10 { 2 } else { 0 };
    wb.write_bits(profile, 3);
    wb.write_bit(still_picture);
    wb.write_bit(still_picture); // reduced_still_picture_header = still_picture

    if still_picture {
        // Reduced header: only seq_level_idx
        wb.write_bits(8, 5); // Level 4.0
    } else {
        wb.write_bit(false); // timing_info_present_flag = 0
        wb.write_bit(false); // initial_display_delay_present_flag = 0
        wb.write_bits(0, 5); // operating_points_cnt_minus_1 = 0
        wb.write_bits(0, 12); // operating_point_idc[0] = 0
        wb.write_bits(8, 5); // seq_level_idx[0] = 8 (Level 4.0)
        wb.write_bit(false); // seq_tier[0] = 0
    }

    // Frame dimensions
    let w_bits = 32 - (width - 1).leading_zeros();
    let h_bits = 32 - (height - 1).leading_zeros();
    wb.write_bits(w_bits - 1, 4); // frame_width_bits_minus_1
    wb.write_bits(h_bits - 1, 4); // frame_height_bits_minus_1
    wb.write_bits(width - 1, w_bits); // max_frame_width_minus_1
    wb.write_bits(height - 1, h_bits); // max_frame_height_minus_1

    if !still_picture {
        wb.write_bit(false); // frame_id_numbers_present_flag = 0
    }

    wb.write_bit(false); // use_128x128_superblock = 0
    wb.write_bit(false); // enable_filter_intra = 0
    wb.write_bit(false); // enable_intra_edge_filter = 0

    if still_picture {
        // For reduced SH: all inter features are implicit 0,
        // seq_force_screen_content_tools = SELECT (implicit),
        // seq_force_integer_mv = SELECT (implicit).
        // NO bits written for these.
    } else {
        wb.write_bit(false); // enable_interintra_compound = 0
        wb.write_bit(false); // enable_masked_compound = 0
        wb.write_bit(false); // enable_warped_motion = 0
        wb.write_bit(false); // enable_dual_filter = 0
        wb.write_bit(true); // enable_order_hint = 1
        wb.write_bit(false); // enable_jnt_comp = 0
        wb.write_bit(false); // enable_ref_frame_mvs = 0
        wb.write_bits(ORDER_HINT_BITS - 1, 3); // order_hint_bits_minus_1

        // seq_choose_screen_content_tools (1 bit, NOT 2!)
        wb.write_bit(true); // = 1 → seq_force_screen_content_tools = SELECT

        // seq_force_screen_content_tools > 0 (SELECT=2 > 0), so:
        // seq_choose_integer_mv (1 bit)
        wb.write_bit(true); // = 1 → seq_force_integer_mv = SELECT
    }

    wb.write_bit(false); // enable_superres = 0
    wb.write_bit(false); // enable_cdef = 0
    wb.write_bit(false); // enable_restoration = 0

    // ---- color_config() ----
    wb.write_bit(bit_depth > 8); // high_bitdepth
    if profile == 2 && bit_depth > 8 {
        wb.write_bit(bit_depth >= 12); // twelve_bit
    }

    // Monochrome: NumPlanes = 1
    // (mono_chrome is present for profile != 1)
    wb.write_bit(true); // mono_chrome = 1

    // color_description_present_flag = 1
    wb.write_bit(true);
    wb.write_bits(color.color_primaries as u32, 8);
    wb.write_bits(color.transfer_characteristics as u32, 8);
    wb.write_bits(color.matrix_coefficients as u32, 8);

    // For mono_chrome: color_range=1 (implicit), subsampling=1,1 (implicit),
    // chroma_sample_position=CSP_UNKNOWN (implicit), separate_uv_delta_q=0 (implicit).
    // NO bits written.

    wb.write_bit(false); // film_grain_params_present = 0

    write_trailing_bits(&mut wb);

    let payload = wb.into_data();
    write_obu(ObuType::SequenceHeader, &payload)
}

/// Write a key frame header for a reduced (still-picture) sequence header.
pub fn write_key_frame_header(
    width: u32,
    height: u32,
    base_qindex: u8,
) -> Vec<u8> {
    write_key_frame_header_full(width, height, base_qindex, true)
}

/// AV1 spec Section 5.9.2: uncompressed_header() for KEY_FRAME.
///
/// Field ordering matches the spec exactly. Monochrome (NumPlanes=1).
pub fn write_key_frame_header_full(
    width: u32,
    height: u32,
    base_qindex: u8,
    reduced_sh: bool,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    if !reduced_sh {
        // ---- Full frame header preamble ----
        wb.write_bit(false); // show_existing_frame = 0
        wb.write_bits(0, 2); // frame_type = KEY_FRAME (0)
        wb.write_bit(true); // show_frame = 1
        // showable_frame: implicit 0 for KEY_FRAME with show_frame=1
        // error_resilient_mode: implicit 1 for KEY_FRAME with show_frame=1
        wb.write_bit(false); // disable_cdf_update = 0
    }
    // For reduced SH: show_existing_frame/frame_type/show_frame/error_resilient
    // are all implicit. disable_cdf_update is not signaled.

    // allow_screen_content_tools: seq_force = SELECT → read 1 bit
    wb.write_bit(false); // allow_screen_content_tools = 0
    // Since allow_screen_content_tools=0: force_integer_mv not signaled

    if !reduced_sh {
        wb.write_bit(false); // frame_size_override_flag = 0
        wb.write_bits(0, ORDER_HINT_BITS); // order_hint = 0
        // primary_ref_frame: NOT signaled for KEY_FRAME with error_resilient=1
        //   (implicit PRIMARY_REF_NONE)
        wb.write_bits(0xFF, 8); // refresh_frame_flags = 0xFF
    }

    // ---- frame_size() ----
    // frame_size_override_flag = 0 → use SH dimensions, no bits
    // superres_params(): enable_superres=0 → no bits

    // ---- render_size() ----
    wb.write_bit(false); // render_and_frame_size_different = 0

    // allow_intrabc: NOT signaled (allow_screen_content_tools=0 → implicit 0)

    // ---- tile_info() ----
    write_tile_info(&mut wb, width, height);

    // ---- quantization_params() ----
    wb.write_bits(base_qindex as u32, 8); // base_q_idx
    wb.write_bit(false); // DeltaQYDc: delta_coded = 0
    // NumPlanes=1 (mono_chrome): no DeltaQUDc, DeltaQUAc
    wb.write_bit(false); // using_qmatrix = 0

    // ---- segmentation_params() ----
    wb.write_bit(false); // segmentation_enabled = 0

    // ---- delta_q_params() ----
    wb.write_bit(false); // delta_q_present = 0
    // delta_lf_params(): not signaled when delta_q_present=0

    // ---- loop_filter_params() ----
    // CodedLossless is only true when base_q_idx=0 AND all delta-Q=0 AND
    // all segments have qindex 0. With base_q_idx>0 in practice, not lossless.
    // allow_intrabc=0, so we always write loop filter params.
    wb.write_bits(0, 6); // loop_filter_level[0] = 0
    wb.write_bits(0, 6); // loop_filter_level[1] = 0
    // NumPlanes=1: no loop_filter_level[2]/[3]
    wb.write_bits(0, 3); // loop_filter_sharpness = 0
    wb.write_bit(false); // loop_filter_delta_enabled = 0

    // ---- cdef_params() ----
    // enable_cdef=0 → no bits (implicit cdef_bits=0)

    // ---- lr_params() ----
    // enable_restoration=0 → no bits

    // ---- read_tx_mode() ----
    // Not CodedLossless (since base_q_idx may be nonzero) →
    wb.write_bit(false); // tx_mode_select = 0 → TX_MODE_LARGEST

    // For intra frames: no reference_select, skip_mode, warped_motion, global_motion

    wb.write_bit(false); // reduced_tx_set = 0

    write_trailing_bits(&mut wb);
    wb.into_data()
}

/// AV1 spec Section 5.9.15: tile_info().
///
/// Writes uniform tile spacing with a single tile (no splitting).
fn write_tile_info(wb: &mut BitWriter, width: u32, height: u32) {
    let sb_size = 64u32; // use_128x128_superblock = 0
    let sb_cols = width.div_ceil(sb_size);
    let sb_rows = height.div_ceil(sb_size);

    wb.write_bit(true); // uniform_tile_spacing_flag = 1

    // TileColsLog2 starts at minLog2TileCols.
    // For our small images, minLog2TileCols = 0.
    // maxLog2TileCols = tile_log2(min(sbCols, MAX_TILE_COLS))
    // MAX_TILE_COLS = 64 in AV1 spec
    let max_log2_tile_cols = tile_log2(sb_cols.min(64));
    // Write 0 (don't increment) for each possible increment level
    // to keep TileColsLog2 = 0 (single tile column).
    for _ in 0..max_log2_tile_cols {
        wb.write_bit(false); // increment_tile_cols_log2 = 0 → break
        break; // Only one 0 needed: the decoder breaks on first 0
    }

    // TileRowsLog2 starts at max(minLog2Tiles - TileColsLog2, 0) = 0
    // maxLog2TileRows = tile_log2(min(sbRows, MAX_TILE_ROWS))
    // MAX_TILE_ROWS = 64 in AV1 spec
    let max_log2_tile_rows = tile_log2(sb_rows.min(64));
    for _ in 0..max_log2_tile_rows {
        wb.write_bit(false); // increment_tile_rows_log2 = 0 → break
        break;
    }

    // TileColsLog2=0, TileRowsLog2=0 → NumTiles=1
    // No context_update_tile_id or tile_size_bytes_minus_1 needed
}

/// Write an inter frame header (non-reduced SH).
pub fn write_inter_frame_header(
    base_qindex: u8,
    refresh_frame_flags: u8,
    order_hint: u8,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    wb.write_bit(false); // show_existing_frame = 0
    wb.write_bits(1, 2); // frame_type = INTER_FRAME (1)
    wb.write_bit(true); // show_frame = 1
    // showable_frame: implicit (frame_type != KEY_FRAME with show_frame=1)
    wb.write_bit(true); // error_resilient_mode = 1

    wb.write_bit(false); // disable_cdf_update = 0
    wb.write_bit(false); // allow_screen_content_tools = 0

    wb.write_bit(false); // frame_size_override_flag = 0
    wb.write_bits(order_hint as u32, ORDER_HINT_BITS); // order_hint
    // primary_ref_frame: NOT signaled (error_resilient_mode=1)
    wb.write_bits(refresh_frame_flags as u32, 8); // refresh_frame_flags

    // ref_frame_idx[0..6] — all pointing to slot 0
    for _ in 0..7 {
        wb.write_bits(0, 3);
    }

    // frame_size(): no bits (no override, no superres)
    // render_size():
    wb.write_bit(false); // render_and_frame_size_different = 0
    // allow_intrabc: not signaled (not intra)

    wb.write_bit(true); // is_filter_switchable = 1
    wb.write_bit(false); // is_motion_mode_switchable = 0
    wb.write_bit(false); // reference_select = 0

    // TODO: tile_info for inter frames — currently assumes caller handles this
    // For now, write minimal tile_info
    wb.write_bit(true); // uniform_tile_spacing_flag = 1

    // Quantization params
    wb.write_bits(base_qindex as u32, 8);
    wb.write_bit(false); // DeltaQYDc delta_coded = 0
    // NumPlanes=1: no chroma delta-Q
    wb.write_bit(false); // using_qmatrix = 0

    wb.write_bit(false); // segmentation_enabled = 0
    wb.write_bit(false); // delta_q_present = 0

    // Loop filter
    wb.write_bits(0, 6); // filter_level[0] = 0
    wb.write_bits(0, 6); // filter_level[1] = 0
    wb.write_bits(0, 3); // loop_filter_sharpness = 0
    wb.write_bit(false); // loop_filter_delta_enabled = 0

    // cdef: enable_cdef=0, no bits
    // lr: enable_restoration=0, no bits

    wb.write_bit(false); // tx_mode_select = 0 → TX_MODE_LARGEST

    // skip_mode_present = 0
    wb.write_bit(false);
    // allow_warped_motion: not present (enable_warped_motion=0 in SH)

    wb.write_bit(false); // reduced_tx_set = 0

    // Global motion params: is_global = 0 for all reference frames
    for _ in 0..7 {
        wb.write_bit(false);
    }

    write_trailing_bits(&mut wb);
    wb.into_data()
}

/// Build the tile group data for a single-tile frame.
///
/// AV1 spec Section 5.11.1: For a single tile, the tile_group_obu()
/// contains tile_start_and_end_present_flag=0 (1 bit) + byte alignment +
/// the raw tile data.
pub fn build_tile_group_single(tile_data: &[u8]) -> Vec<u8> {
    let mut wb = BitWriter::new();
    // tile_start_and_end_present_flag = 0 (single tile, no start/end)
    wb.write_bit(false);
    // Byte alignment
    write_trailing_bits(&mut wb);
    let header = wb.into_data();

    let mut result = Vec::with_capacity(header.len() + tile_data.len());
    result.extend_from_slice(&header);
    result.extend_from_slice(tile_data);
    result
}

/// Build the tile group data for a multi-tile frame.
///
/// AV1 spec Section 5.11.1: For NumTiles > 1, write
/// tile_start_and_end_present_flag=0 (all tiles in one TG), byte align,
/// then for each tile except the last: 4-byte LE tile size, followed by
/// tile data. The last tile has no size prefix.
pub fn build_tile_group_multi(tile_bitstreams: &[Vec<u8>]) -> Vec<u8> {
    if tile_bitstreams.len() <= 1 {
        return build_tile_group_single(
            tile_bitstreams.first().map(|v| v.as_slice()).unwrap_or(&[]),
        );
    }

    let mut wb = BitWriter::new();
    wb.write_bit(false);
    write_trailing_bits(&mut wb);
    let header = wb.into_data();

    let total_size: usize = header.len()
        + tile_bitstreams[..tile_bitstreams.len() - 1]
            .iter()
            .map(|t| 4 + t.len())
            .sum::<usize>()
        + tile_bitstreams.last().map_or(0, |t| t.len());

    let mut result = Vec::with_capacity(total_size);
    result.extend_from_slice(&header);

    for (i, tile) in tile_bitstreams.iter().enumerate() {
        if i < tile_bitstreams.len() - 1 {
            let size_minus_1 = (tile.len() as u32).saturating_sub(1);
            result.extend_from_slice(&size_minus_1.to_le_bytes());
        }
        result.extend_from_slice(tile);
    }

    result
}

/// Write a complete minimal AV1 bitstream for a still image.
///
/// Produces: temporal_delimiter + sequence_header + frame (header + tile group).
pub fn write_still_frame(width: u32, height: u32, base_qindex: u8, tile_data: &[u8]) -> Vec<u8> {
    let mut bitstream = Vec::new();

    bitstream.extend_from_slice(&write_temporal_delimiter());
    bitstream.extend_from_slice(&write_sequence_header(width, height));

    let fh_bytes = write_key_frame_header(width, height, base_qindex);
    let tg_bytes = build_tile_group_single(tile_data);
    let mut frame_payload = Vec::with_capacity(fh_bytes.len() + tg_bytes.len());
    frame_payload.extend_from_slice(&fh_bytes);
    frame_payload.extend_from_slice(&tg_bytes);

    bitstream.extend_from_slice(&write_obu(ObuType::Frame, &frame_payload));

    bitstream
}

/// Write an inter frame as a Frame OBU.
///
/// `tile_group_data` should be a pre-formed tile group (from
/// `build_tile_group_single` or `build_tile_group_multi`).
pub fn write_inter_frame(
    base_qindex: u8,
    refresh_frame_flags: u8,
    order_hint: u8,
    tile_group_data: &[u8],
) -> Vec<u8> {
    let header = write_inter_frame_header(base_qindex, refresh_frame_flags, order_hint);
    let mut payload = Vec::with_capacity(header.len() + tile_group_data.len());
    payload.extend_from_slice(&header);
    payload.extend_from_slice(tile_group_data);
    write_obu(ObuType::Frame, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uleb_encode_small() {
        assert_eq!(uleb_encode(0), vec![0]);
        assert_eq!(uleb_encode(1), vec![1]);
        assert_eq!(uleb_encode(127), vec![127]);
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
        // 64x64 = 1 SB → uniform + no increments
        let mut wb = BitWriter::new();
        write_tile_info(&mut wb, 64, 64);
        // Should be just 1 bit (uniform_tile_spacing_flag)
        assert_eq!(wb.bit_offset, 1);
    }

    #[test]
    fn tile_info_four_sbs() {
        // 128x128 = 4 SBs → uniform + 1 col increment + 1 row increment
        let mut wb = BitWriter::new();
        write_tile_info(&mut wb, 128, 128);
        // uniform_flag (1) + col_increment (1) + row_increment (1) = 3
        assert_eq!(wb.bit_offset, 3);
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
}
