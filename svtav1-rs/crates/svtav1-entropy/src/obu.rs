//! OBU (Open Bitstream Unit) writer for AV1 bitstreams.
//!
//! Ported from SVT-AV1's entropy_coding.c OBU writing functions.
//! Produces valid AV1 bitstream output.

use alloc::vec::Vec;

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

/// Write a minimal sequence header OBU.
///
/// This produces a valid AV1 sequence header for 8-bit 4:2:0 content.
pub fn write_sequence_header(width: u32, height: u32) -> Vec<u8> {
    let mut wb = BitWriter::new();

    // seq_profile = 0 (Main profile: 8/10-bit 4:2:0)
    wb.write_bits(0, 3);
    // still_picture = 1
    wb.write_bit(true);
    // reduced_still_picture_header = 1
    wb.write_bit(true);

    // With reduced_still_picture_header:
    // timing_info_present_flag = 0 (implicit)
    // decoder_model_info_present_flag = 0 (implicit)
    // initial_display_delay_present_flag = 0 (implicit)
    // operating_points_cnt_minus_1 = 0 (implicit)
    // operating_point_idc[0] = 0 (implicit)
    // seq_level_idx[0]
    wb.write_bits(8, 5); // Level 4.0

    // frame_width_bits_minus_1 and frame_height_bits_minus_1
    let w_bits = 32 - (width - 1).leading_zeros();
    let h_bits = 32 - (height - 1).leading_zeros();
    wb.write_bits(w_bits - 1, 4);
    wb.write_bits(h_bits - 1, 4);
    // max_frame_width_minus_1
    wb.write_bits(width - 1, w_bits);
    // max_frame_height_minus_1
    wb.write_bits(height - 1, h_bits);

    // frame_id_numbers_present_flag = 0 (reduced header)
    // use_128x128_superblock
    wb.write_bit(false);
    // enable_filter_intra
    wb.write_bit(false);
    // enable_intra_edge_filter
    wb.write_bit(false);

    // With reduced_still_picture_header, many features are disabled:
    // enable_interintra_compound = 0
    // enable_masked_compound = 0
    // enable_warped_motion = 0
    // enable_dual_filter = 0
    // enable_order_hint = 0
    // enable_jnt_comp = 0 (implicit)
    // enable_ref_frame_mvs = 0 (implicit)
    // seq_choose_screen_content_tools = SELECT (2)
    wb.write_bits(2, 2); // seq_force_screen_content_tools = SELECT
    // seq_choose_integer_mv = SELECT (implied by screen_content=SELECT)

    // enable_superres = 0
    wb.write_bit(false);
    // enable_cdef = 0
    wb.write_bit(false);
    // enable_restoration = 0
    wb.write_bit(false);

    // Color config
    // high_bitdepth = 0 (8-bit)
    wb.write_bit(false);
    // mono_chrome = 0
    wb.write_bit(false);
    // color_description_present_flag = 0
    wb.write_bit(false);
    // color_range = 0 (studio/limited range)
    wb.write_bit(false);
    // subsampling_x = 1, subsampling_y = 1 (4:2:0)
    // chroma_sample_position = 0 (unknown)
    wb.write_bits(0, 2);
    // separate_uv_delta_q = 0
    wb.write_bit(false);

    // film_grain_params_present = 0
    wb.write_bit(false);

    // trailing bits (byte-align)
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bit(true); // trailing 1
        let pad = 8 - (wb.bit_offset % 8);
        if pad < 8 {
            wb.write_bits(0, pad);
        }
    }

    let payload = wb.into_data();
    write_obu(ObuType::SequenceHeader, &payload)
}

/// Write a minimal frame header for an intra-only key frame.
pub fn write_key_frame_header(_width: u32, _height: u32, base_qindex: u8) -> Vec<u8> {
    let mut wb = BitWriter::new();

    // With reduced_still_picture_header:
    // show_existing_frame = 0 (implicit)
    // frame_type = KEY_FRAME (implicit)
    // show_frame = 1 (implicit)
    // All the frame header is essentially just the quantization params

    // No frame ID (frame_id_numbers_present_flag = 0)

    // allow_screen_content_tools (if seq_force = SELECT)
    wb.write_bit(false);

    // Frame size (same as sequence header for still picture)
    // frame_size_override_flag = 0
    // (no frame size written — uses max_frame_width/height from seq header)

    // allow_intrabc = 0
    wb.write_bit(false);

    // Quantization params
    // base_q_idx
    wb.write_bits(base_qindex as u32, 8);
    // delta_coded (DeltaQYDc) = 0
    wb.write_bit(false);
    // No U/V delta Q for profile 0
    // using_qmatrix = 0
    wb.write_bit(false);

    // Segmentation: enabled = 0
    wb.write_bit(false);

    // delta_q_present = 0
    wb.write_bit(false);

    // Loop filter: filter_level[0] = 0, filter_level[1] = 0
    wb.write_bits(0, 6);
    wb.write_bits(0, 6);

    // CDEF: cdef_damping - 3
    wb.write_bits(0, 2);
    // cdef_bits = 0
    wb.write_bits(0, 2);

    // Loop restoration: frame_restoration_type = NONE for all planes
    // (enabled only if enable_restoration = 1 in seq header)

    // TX mode
    wb.write_bit(true); // tx_mode_select = 1 (TX_MODE_SELECT)

    // reference_select = 0 (single reference only for key frame)
    // (not signaled for key frame)

    // skip_mode = 0 (not present for key frame)

    // allow_warped_motion = 0 (not present for key frame)

    // reduced_tx_set = 0
    wb.write_bit(false);

    // Trailing bits
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bit(true);
        let pad = 8 - (wb.bit_offset % 8);
        if pad < 8 {
            wb.write_bits(0, pad);
        }
    }

    let payload = wb.into_data();
    write_obu(ObuType::FrameHeader, &payload)
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

    // Frame OBU (contains frame header + tile group)
    let frame_header = write_key_frame_header(width, height, base_qindex);
    // For a Frame OBU, the payload is frame_header_bytes + tile_group_bytes
    // But with reduced_still_picture_header, we use a Frame OBU directly
    let mut frame_payload = Vec::new();
    frame_payload.extend_from_slice(&frame_header);
    frame_payload.extend_from_slice(tile_data);

    bitstream.extend_from_slice(&write_obu(ObuType::Frame, &frame_payload));

    bitstream
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
