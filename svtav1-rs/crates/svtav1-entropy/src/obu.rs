//! OBU (Open Bitstream Unit) writer for AV1 bitstreams.
//!
//! Spec 07 §5.3: OBU bitstream format.
//!
//! Ported from SVT-AV1's entropy_coding.c OBU writing functions.
//! Produces valid AV1 bitstream output.

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

/// Write trailing bits to byte-align the bitwriter.
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

fn write_sequence_header_inner(
    width: u32,
    height: u32,
    still_picture: bool,
    bit_depth: u8,
    color: &ColorDescription,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    // seq_profile: 0 = Main (8/10-bit 4:2:0), 1 = High (4:4:4), 2 = Professional (12-bit)
    let profile = if bit_depth > 10 { 2 } else { 0 };
    wb.write_bits(profile, 3);
    // still_picture
    wb.write_bit(still_picture);
    // reduced_still_picture_header
    wb.write_bit(still_picture);

    if still_picture {
        // Reduced header: only seq_level_idx
        wb.write_bits(8, 5); // Level 4.0
    } else {
        // Full header
        // timing_info_present_flag = 0
        wb.write_bit(false);
        // initial_display_delay_present_flag = 0
        wb.write_bit(false);
        // operating_points_cnt_minus_1 = 0
        wb.write_bits(0, 5);
        // operating_point_idc[0] = 0
        wb.write_bits(0, 12);
        // seq_level_idx[0] = 8 (Level 4.0)
        wb.write_bits(8, 5);
        // seq_tier[0] = 0 (since level > 7)
        wb.write_bit(false);
    }

    // frame_width_bits_minus_1 and frame_height_bits_minus_1
    let w_bits = 32 - (width - 1).leading_zeros();
    let h_bits = 32 - (height - 1).leading_zeros();
    wb.write_bits(w_bits - 1, 4);
    wb.write_bits(h_bits - 1, 4);
    // max_frame_width_minus_1
    wb.write_bits(width - 1, w_bits);
    // max_frame_height_minus_1
    wb.write_bits(height - 1, h_bits);

    if !still_picture {
        // frame_id_numbers_present_flag = 0
        wb.write_bit(false);
    }

    // use_128x128_superblock = 0
    wb.write_bit(false);
    // enable_filter_intra = 0
    wb.write_bit(false);
    // enable_intra_edge_filter = 0
    wb.write_bit(false);

    if !still_picture {
        // enable_interintra_compound = 0
        wb.write_bit(false);
        // enable_masked_compound = 0
        wb.write_bit(false);
        // enable_warped_motion = 0
        wb.write_bit(false);
        // enable_dual_filter = 0
        wb.write_bit(false);
        // enable_order_hint = 1 (needed for reference management)
        wb.write_bit(true);
        // enable_jnt_comp = 0 (requires order_hint)
        wb.write_bit(false);
        // enable_ref_frame_mvs = 0 (requires order_hint)
        wb.write_bit(false);
        // order_hint_bits_minus_1
        wb.write_bits(ORDER_HINT_BITS - 1, 3);
    }

    // seq_choose_screen_content_tools = SELECT (2)
    wb.write_bits(2, 2);
    // seq_choose_integer_mv: when screen_content=SELECT, this is also SELECT
    // For non-still-picture: need to write seq_force_integer_mv
    if !still_picture {
        wb.write_bits(2, 2); // seq_force_integer_mv = SELECT
    }

    // enable_superres = 0
    wb.write_bit(false);
    // enable_cdef = 0
    wb.write_bit(false);
    // enable_restoration = 0
    wb.write_bit(false);

    // Color config (AV1 spec Section 5.5.2)
    // high_bitdepth
    wb.write_bit(bit_depth > 8);
    if profile == 2 && bit_depth > 8 {
        // twelve_bit flag (only for Professional profile)
        wb.write_bit(bit_depth >= 12);
    }
    // mono_chrome = 0 (not monochrome)
    wb.write_bit(false);
    // color_description_present_flag = 1 (signal CICP)
    wb.write_bit(true);
    // color_primaries (8 bits)
    wb.write_bits(color.color_primaries as u32, 8);
    // transfer_characteristics (8 bits)
    wb.write_bits(color.transfer_characteristics as u32, 8);
    // matrix_coefficients (8 bits)
    wb.write_bits(color.matrix_coefficients as u32, 8);
    // color_range
    wb.write_bit(color.full_range);
    // subsampling_x = 1, subsampling_y = 1 (4:2:0)
    if color.matrix_coefficients == 0 {
        // Identity matrix (RGB): no subsampling for profile 1+
        if profile > 0 {
            wb.write_bit(false); // subsampling_x = 0
            wb.write_bit(false); // subsampling_y = 0
        }
    } else {
        // chroma_sample_position = 0 (unknown) for 4:2:0
        wb.write_bits(0, 2);
    }
    // separate_uv_delta_q = 0
    wb.write_bit(false);

    // film_grain_params_present = 0
    wb.write_bit(false);

    write_trailing_bits(&mut wb);

    let payload = wb.into_data();
    write_obu(ObuType::SequenceHeader, &payload)
}

/// Write a minimal frame header for an intra-only key frame.
/// Write a key frame header compatible with either reduced or full SH.
///
/// When `reduced_sh` is true, uses the minimal reduced-still-picture format.
/// When false, writes a full key frame header with show_existing_frame,
/// frame_type, refresh_frame_flags, and order_hint.
pub fn write_key_frame_header(
    _width: u32,
    _height: u32,
    base_qindex: u8,
) -> Vec<u8> {
    write_key_frame_header_full(_width, _height, base_qindex, true)
}

/// Write a key frame header for a full (non-reduced) sequence header.
pub fn write_key_frame_header_full(
    _width: u32,
    _height: u32,
    base_qindex: u8,
    reduced_sh: bool,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    if !reduced_sh {
        // Full frame header for non-reduced SH
        // show_existing_frame = 0
        wb.write_bit(false);
        // frame_type = KEY_FRAME (0)
        wb.write_bits(0, 2);
        // show_frame = 1
        wb.write_bit(true);
        // showable_frame: not present for KEY_FRAME with show_frame=1
        // error_resilient_mode = 1 (implicit for KEY_FRAME)
    }

    // disable_cdf_update = 0 (only for non-reduced)
    if !reduced_sh {
        wb.write_bit(false);
    }

    // allow_screen_content_tools (if seq_force = SELECT)
    wb.write_bit(false);

    if !reduced_sh {
        // frame_size_override_flag = 0
        wb.write_bit(false);
        // order_hint = 0 (key frame is always order 0 or start of new sequence)
        wb.write_bits(0, ORDER_HINT_BITS);
        // primary_ref_frame = 7 (PRIMARY_REF_NONE)
        wb.write_bits(7, 3);
        // refresh_frame_flags = 0xFF (refresh all slots)
        wb.write_bits(0xFF, 8);
    }

    // allow_intrabc = 0
    wb.write_bit(false);

    // Quantization params
    wb.write_bits(base_qindex as u32, 8);
    // delta_coded (DeltaQYDc) = 0
    wb.write_bit(false);
    // using_qmatrix = 0
    wb.write_bit(false);

    // Segmentation: enabled = 0
    wb.write_bit(false);

    // delta_q_present = 0
    wb.write_bit(false);

    // Loop filter: filter_level[0] = 0, filter_level[1] = 0
    wb.write_bits(0, 6);
    wb.write_bits(0, 6);

    // CDEF: cdef_damping - 3 = 0, cdef_bits = 0
    wb.write_bits(0, 2);
    wb.write_bits(0, 2);

    // TX mode: tx_mode_select = 1 (TX_MODE_SELECT)
    wb.write_bit(true);

    // reduced_tx_set = 0
    wb.write_bit(false);

    write_trailing_bits(&mut wb);

    // Return raw header bytes (not wrapped in OBU).
    // The caller combines this with tile data into a Frame OBU (type 6).
    wb.into_data()
}

/// Write a minimal inter frame header.
///
/// Produces a frame header for an inter (non-key) frame with:
/// - frame_type = INTER_FRAME
/// - show_frame = 1
/// - error_resilient_mode = 1 (simplified)
/// - refresh_frame_flags
/// - base_q_idx
pub fn write_inter_frame_header(
    base_qindex: u8,
    refresh_frame_flags: u8,
    order_hint: u8,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    // show_existing_frame = 0
    wb.write_bit(false);
    // frame_type = INTER_FRAME (1)
    wb.write_bits(1, 2);
    // show_frame = 1
    wb.write_bit(true);
    // showable_frame = 1 (implicit for show_frame=1 non-key)
    // error_resilient_mode = 1 (simplifies reference management)
    wb.write_bit(true);

    // disable_cdf_update = 0
    wb.write_bit(false);
    // allow_screen_content_tools = 0
    wb.write_bit(false);

    // frame_size_override_flag = 0 (use SH dimensions)
    wb.write_bit(false);

    // order_hint (enable_order_hint = 1 in full SH)
    wb.write_bits(order_hint as u32, ORDER_HINT_BITS);

    // primary_ref_frame = 7 (PRIMARY_REF_NONE — no previous context to load)
    wb.write_bits(7, 3);

    // refresh_frame_flags
    wb.write_bits(refresh_frame_flags as u32, 8);

    // ref_frame_idx[0..6] — all pointing to slot 0 for simplicity
    for _ in 0..7 {
        wb.write_bits(0, 3);
    }

    // allow_intrabc = 0
    // (not signaled for inter frames without screen content tools)

    // interpolation_filter: SWITCHABLE (2 bits = 4)
    wb.write_bit(true); // is_filter_switchable = 1

    // is_motion_mode_switchable = 0
    wb.write_bit(false);

    // reference_select = 0 (single reference)
    wb.write_bit(false);

    // Quantization params
    wb.write_bits(base_qindex as u32, 8);
    wb.write_bit(false); // delta_coded DeltaQYDc = 0
    wb.write_bit(false); // using_qmatrix = 0

    // Segmentation: enabled = 0
    wb.write_bit(false);

    // delta_q_present = 0
    wb.write_bit(false);

    // Loop filter: filter_level[0] = 0, filter_level[1] = 0
    wb.write_bits(0, 6);
    wb.write_bits(0, 6);

    // CDEF
    wb.write_bits(0, 2); // cdef_damping - 3
    wb.write_bits(0, 2); // cdef_bits

    // TX mode
    wb.write_bit(true); // tx_mode_select = 1

    // skip_mode_present = 0
    wb.write_bit(false);

    // allow_warped_motion = 0
    wb.write_bit(false);

    // reduced_tx_set = 0
    wb.write_bit(false);

    // Global motion params: is_global = 0 for all reference frames
    for _ in 0..7 {
        wb.write_bit(false); // is_global
    }

    // Trailing bits
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bit(true);
        let pad = 8 - (wb.bit_offset % 8);
        if pad < 8 {
            wb.write_bits(0, pad);
        }
    }

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
    // tile_start_and_end_present_flag = 0 (all tiles in this TG)
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

    // Each tile except the last is preceded by its size (4 bytes LE)
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

    // Temporal delimiter
    bitstream.extend_from_slice(&write_temporal_delimiter());

    // Sequence header
    bitstream.extend_from_slice(&write_sequence_header(width, height));

    // Frame OBU (type 6): raw frame header bytes + tile group data
    let fh_bytes = write_key_frame_header(width, height, base_qindex);
    let tg_bytes = build_tile_group_single(tile_data);
    let mut frame_payload = Vec::with_capacity(fh_bytes.len() + tg_bytes.len());
    frame_payload.extend_from_slice(&fh_bytes);
    frame_payload.extend_from_slice(&tg_bytes);

    bitstream.extend_from_slice(&write_obu(ObuType::Frame, &frame_payload));

    bitstream
}

/// Write an inter frame as a Frame OBU (frame header + tile group).
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
        // Byte: 0 | 0001 (seq header=1) | 0 | 1 | 0 = 0b0_0001_0_1_0 = 0x0A
        assert_eq!(header[0], 0b0_0001_0_1_0);
    }

    #[test]
    fn obu_header_frame() {
        let header = write_obu_header(ObuType::Frame, false);
        // 0 | 0110 (frame=6) | 0 | 1 | 0 = 0b0_0110_0_1_0 = 0x32
        assert_eq!(header[0], 0b0_0110_0_1_0);
    }

    #[test]
    fn temporal_delimiter_obu() {
        let td = write_temporal_delimiter();
        // Header (1 byte) + size (1 byte, value 0) = 2 bytes
        assert_eq!(td.len(), 2);
        // Type = 2: 0b0_0010_0_1_0 = 0x12
        assert_eq!(td[0], 0b0_0010_0_1_0);
        assert_eq!(td[1], 0); // payload size = 0
    }

    #[test]
    fn sequence_header_non_empty() {
        let sh = write_sequence_header(64, 64);
        assert!(sh.len() > 3, "sequence header should be > 3 bytes");
        // First byte is OBU header for sequence header
        assert_eq!(sh[0], 0b0_0001_0_1_0);
    }

    #[test]
    fn still_frame_produces_valid_structure() {
        let tile_data = vec![0u8; 10]; // dummy tile data
        let bitstream = write_still_frame(64, 64, 128, &tile_data);
        assert!(bitstream.len() > 20, "bitstream should be substantial");
        // Should start with temporal delimiter
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
}
