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
#[derive(Debug, Clone, Copy)]
pub struct ColorDescription {
    /// Color primaries (1=BT.709/sRGB, 2=unspecified, 9=BT.2020, 12=P3).
    pub color_primaries: u8,
    /// Transfer characteristics (1=BT.709, 2=unspecified, 13=sRGB,
    /// 16=PQ/HDR10, 18=HLG).
    pub transfer_characteristics: u8,
    /// Matrix coefficients (1=BT.709, 2=unspecified, 9=BT.2020,
    /// 0=Identity/RGB).
    pub matrix_coefficients: u8,
    /// Full range (true) or limited/studio range (false).
    pub full_range: bool,
}

/// The default is [`ColorDescription::unspecified`] — the C encoder's
/// default color configuration (`enc_settings.c:1043-1046`), which the SH
/// writer signals as `color_description_present_flag = 0`.
impl Default for ColorDescription {
    fn default() -> Self {
        Self::unspecified()
    }
}

impl ColorDescription {
    /// CICP "unspecified": cp/tc/mc = 2/2/2, studio (limited) range.
    ///
    /// These are the C encoder's defaults (`svt_av1_set_default_params`,
    /// `Source/Lib/Globals/enc_settings.c:1043-1046`: cp/tc/mc = 2 and
    /// `color_range = EB_CR_STUDIO_RANGE`). The SH writer emits
    /// `color_description_present_flag = 0` (no CICP bytes) for this
    /// value, exactly like C's `write_color_config`
    /// (`Source/Lib/Codec/entropy_coding.c:2749-2753`).
    pub fn unspecified() -> Self {
        Self {
            color_primaries: 2,
            transfer_characteristics: 2,
            matrix_coefficients: 2,
            full_range: false,
        }
    }

    /// True iff cp/tc/mc are all 2 ("unspecified"), in which case the SH
    /// carries no color description (C `write_color_config` behavior).
    pub fn is_unspecified(&self) -> bool {
        self.color_primaries == 2
            && self.transfer_characteristics == 2
            && self.matrix_coefficients == 2
    }
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

/// Sequence-level tool bits the SH signals that vary per encoder preset.
///
/// C derives these once per sequence in `svt_aom_sig_deriv_pre_analysis_scs`
/// (`Source/Lib/Codec/enc_mode_config.c`): `seq_header.filter_intra_level`
/// (:4017-4025) and `seq_header.enable_restoration` (:4051-4071). The
/// encoder crate computes them with its C-exact per-preset port
/// (`seq_tools_for_preset`) and threads them into
/// [`write_sequence_header_ex`]; the FH writer needs `enable_restoration`
/// too because it gates the lr_params() walk (spec 5.9.20).
///
/// The default is both off — matching every allintra preset >= M7 (M10/M13
/// were byte-identical to C with these bits hardwired 0) and the mono
/// convenience wrappers' historical behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeqTools {
    /// SH `enable_filter_intra` (spec 5.5.1): gates the per-block
    /// `use_filter_intra` symbol for eligible intra blocks
    /// ([`crate::context::write_use_filter_intra`]).
    pub enable_filter_intra: bool,
    /// SH `enable_intra_edge_filter` (spec 5.5.1): the DECODER then
    /// filters/upsamples directional-prediction edges
    /// (libaom build_intra_predictors `disable_edge_filter`), so the
    /// encoder's reconstruction path must do the same. C allintra
    /// derivation (`svt_aom_sig_deriv_pre_analysis_scs`,
    /// enc_mode_config.c:4036-4048): 1 iff `dist_based_ang_intra_level
    /// >= 1 || angular_pred_level[intra_level] in {2, 3}` — true ONLY at
    /// M5 (intra_level 2 -> angular_pred_level 2) among representable
    /// allintra presets.
    pub enable_intra_edge_filter: bool,
    /// SH `enable_restoration` (spec 5.5.1): gates the FH lr_params()
    /// fields (spec 5.9.20) — every frame header of the sequence must
    /// then carry per-plane `lr_type` bits.
    pub enable_restoration: bool,
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
///
/// Convenience wrapper: explicit sRGB CICP, 30 fps level derivation,
/// preset-independent tools off ([`SeqTools::default`]).
pub fn write_sequence_header(width: u32, height: u32) -> Vec<u8> {
    write_sequence_header_ex(
        width,
        height,
        true,
        8,
        &ColorDescription::srgb(),
        true,
        30.0,
        SeqTools::default(),
    )
}

/// Write a full sequence header OBU that supports inter frames.
///
/// Convenience wrapper: explicit sRGB CICP, 30 fps level derivation,
/// preset-independent tools off ([`SeqTools::default`]).
pub fn write_sequence_header_full(width: u32, height: u32) -> Vec<u8> {
    write_sequence_header_ex(
        width,
        height,
        false,
        8,
        &ColorDescription::srgb(),
        true,
        30.0,
        SeqTools::default(),
    )
}

/// Write a sequence header with explicit bit depth and color description.
///
/// Supports 8, 10, or 12 bit depth and CICP color signaling for
/// wide gamut (P3, Rec.2020) and HDR (PQ, HLG).
///
/// `monochrome = true` writes the spec 5.5.2 mono color_config (NumPlanes=1,
/// luma-only streams); `monochrome = false` writes the profile-0 4:2:0
/// color_config (NumPlanes=3).
///
/// `fps` feeds the C-exact `seq_level_idx` auto-derivation
/// ([`compute_seq_level_idx`]); C uses `scs->frame_rate` =
/// numerator/denominator of the configured frame rate.
///
/// `tools` carries the per-preset SH tool bits (`enable_filter_intra` /
/// `enable_restoration`) — see [`SeqTools`]. Signaling
/// `enable_restoration` obligates every FH of the sequence to carry
/// lr_params(), and `enable_filter_intra` obligates the tile walk to code
/// `use_filter_intra` for eligible blocks: callers must thread the SAME
/// bits to [`write_key_frame_header_full`] and the entropy walk.
#[allow(clippy::too_many_arguments)]
pub fn write_sequence_header_ex(
    width: u32,
    height: u32,
    still_picture: bool,
    bit_depth: u8,
    color: &ColorDescription,
    monochrome: bool,
    fps: f64,
    tools: SeqTools,
) -> Vec<u8> {
    write_sequence_header_inner(
        width,
        height,
        still_picture,
        bit_depth,
        color,
        monochrome,
        fps,
        tools,
    )
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

/// C `does_level_match` (`Source/Lib/Codec/entropy_coding.c:101-110`):
/// dims/display-sample-rate check for one level ladder entry.
fn does_level_match(
    width: u32,
    height: u32,
    fps: f64,
    lvl_width: u32,
    lvl_height: u32,
    lvl_fps: f64,
    lvl_dim_mult: u32,
) -> bool {
    let lvl_luma_pels = i64::from(lvl_width) * i64::from(lvl_height);
    let lvl_display_sample_rate = lvl_luma_pels as f64 * lvl_fps;
    let luma_pels = i64::from(width) * i64::from(height);
    let display_sample_rate = luma_pels as f64 * fps;
    luma_pels <= lvl_luma_pels
        && display_sample_rate <= lvl_display_sample_rate
        && width <= lvl_width * lvl_dim_mult
        && height <= lvl_height * lvl_dim_mult
}

/// Auto-compute `seq_level_idx` from frame dimensions and frame rate.
///
/// Port of C `set_bitstream_level_tier`
/// (`Source/Lib/Codec/entropy_coding.c:111-232`, the `static_config.level
/// == 0` auto branch — the C default) followed by
/// `major_minor_to_seq_level_idx` (`entropy_coding.h:85-88`):
/// `((major - LEVEL_MAJOR_MIN) << LEVEL_MINOR_BITS) + minor` with
/// `LEVEL_MAJOR_MIN = 2`, `LEVEL_MINOR_BITS = 2` (`definitions.h:398-401`).
/// The C ladder only checks dims + display sample rate (its own comment:
/// bit rate / header rate checks not covered). Falls through to level
/// "maximum parameters" {9,3} → idx 31 when nothing matches.
///
/// 64x64 stills at 30 fps land on the first rung: level 2.0 → idx 0.
pub fn compute_seq_level_idx(width: u32, height: u32, fps: f64) -> u8 {
    // (lvl_width, lvl_height, lvl_fps, lvl_dim_mult, major, minor)
    const LADDER: [(u32, u32, f64, u32, u8, u8); 12] = [
        (512, 288, 30.0, 4, 2, 0),
        (704, 396, 30.0, 4, 2, 1),
        (1088, 612, 30.0, 4, 3, 0),
        (1376, 774, 30.0, 4, 3, 1),
        (2048, 1152, 30.0, 3, 4, 0),
        (2048, 1152, 60.0, 3, 4, 1),
        (4096, 2176, 30.0, 2, 5, 0),
        (4096, 2176, 60.0, 2, 5, 1),
        (4096, 2176, 120.0, 2, 5, 2),
        (8192, 4352, 30.0, 2, 6, 0),
        (8192, 4352, 60.0, 2, 6, 1),
        (8192, 4352, 120.0, 2, 6, 2),
    ];
    let (mut major, mut minor) = (9u8, 3u8); // C default bl = {9, 3}
    for &(lw, lh, lfps, mult, maj, min) in &LADDER {
        if does_level_match(width, height, fps, lw, lh, lfps, mult) {
            (major, minor) = (maj, min);
            break;
        }
    }
    ((major - 2) << 2) + minor
}

/// Compute ceil(log2(n)), with tile_log2(0) = 0, tile_log2(1) = 0.
fn tile_log2(n: u32) -> u32 {
    if n <= 1 {
        return 0;
    }
    32 - (n - 1).leading_zeros()
}

/// AV1 spec Section 5.5.1: Sequence header OBU.
///
/// `monochrome = true`: NumPlanes=1 (luma-only encoder output).
/// `monochrome = false`: profile-0 4:2:0, NumPlanes=3.
#[allow(clippy::too_many_arguments)]
fn write_sequence_header_inner(
    width: u32,
    height: u32,
    still_picture: bool,
    bit_depth: u8,
    color: &ColorDescription,
    monochrome: bool,
    fps: f64,
    tools: SeqTools,
) -> Vec<u8> {
    let mut wb = BitWriter::new();

    // seq_profile: 0 = Main (8/10-bit 4:2:0), 2 = Professional (12-bit)
    let profile = if bit_depth > 10 { 2 } else { 0 };
    wb.write_bits(profile, 3);
    wb.write_bit(still_picture);
    wb.write_bit(still_picture); // reduced_still_picture_header = still_picture

    // C-exact auto level (set_bitstream_level_tier; 64x64@30 → 2.0 → 0).
    let seq_level_idx = compute_seq_level_idx(width, height, fps);

    if still_picture {
        // Reduced header: only seq_level_idx
        wb.write_bits(seq_level_idx as u32, 5);
    } else {
        wb.write_bit(false); // timing_info_present_flag = 0
        wb.write_bit(false); // initial_display_delay_present_flag = 0
        wb.write_bits(0, 5); // operating_points_cnt_minus_1 = 0
        wb.write_bits(0, 12); // operating_point_idc[0] = 0
        wb.write_bits(seq_level_idx as u32, 5); // seq_level_idx[0]
        // seq_tier is only coded for seq_level_idx > 7 (level major > 3):
        // spec 5.5.1 and C write_sequence_header_obu
        // (entropy_coding.c:3790-3792, `if (scs->level[i].major > 3)`).
        if seq_level_idx > 7 {
            wb.write_bit(false); // seq_tier[0] = 0 (main tier)
        }
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
    // enable_filter_intra: per-preset in C —
    // scs->seq_header.filter_intra_level = (allintra level != 0), set by
    // get_filter_intra_level_allintra (enc_mode_config.c:12679, on for
    // <= M6) via enc_mode_config.c:4017-4025; written verbatim by
    // write_sequence_header_obu (entropy_coding.c:2850).
    wb.write_bit(tools.enable_filter_intra);
    wb.write_bit(tools.enable_intra_edge_filter); // enable_intra_edge_filter

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
    // enable_cdef = 1: matches C (scs->seq_header.cdef_level defaults on;
    // the C SH golden below carries the same bit). Every frame header now
    // carries cdef_params() (spec 5.9.19) — zero strengths when CDEF is off
    // for the frame, which a conforming decoder treats as "no CDEF pass"
    // (libaom do_cdef gate, decodeframe.c:5417).
    wb.write_bit(true); // enable_cdef = 1
    // enable_restoration: per-preset in C —
    // svt_aom_get_enable_restoration_allintra (enc_mode_config.c:3944,
    // wn>0 || sg>0 at DEFAULT config → on for <= M6) assigned at
    // enc_mode_config.c:4057; written by write_sequence_header_obu
    // (entropy_coding.c:2891). When set, every FH carries lr_params()
    // (spec 5.9.20) — write_key_frame_header_full must get the same bit.
    wb.write_bit(tools.enable_restoration);

    // ---- color_config() ----
    // Spec 5.5.2; decoder authority: libaom av1_read_color_config
    // (av1/decoder/decodeframe.c:4167); C write path: write_color_config
    // (Source/Lib/Codec/entropy_coding.c:2740).
    wb.write_bit(bit_depth > 8); // high_bitdepth
    if profile == 2 && bit_depth > 8 {
        wb.write_bit(bit_depth >= 12); // twelve_bit
    }

    // mono_chrome: NumPlanes = mono_chrome ? 1 : 3
    // (present for profile != 1)
    wb.write_bit(monochrome);

    // color_description_present_flag: 0 when cp/tc/mc are all
    // "unspecified" (2/2/2) — C write_color_config
    // (entropy_coding.c:2749-2758) — else 1 followed by the three bytes.
    if color.is_unspecified() {
        wb.write_bit(false); // no color description
    } else {
        wb.write_bit(true);
        wb.write_bits(color.color_primaries as u32, 8);
        wb.write_bits(color.transfer_characteristics as u32, 8);
        wb.write_bits(color.matrix_coefficients as u32, 8);
    }

    if monochrome {
        // For mono_chrome, spec 5.5.2 reads color_range (1 bit) and then
        // stops: subsampling=1,1, chroma_sample_position=CSP_UNKNOWN, and
        // separate_uv_delta_q=0 are implicit — but color_range is NOT.
        wb.write_bit(color.full_range); // color_range
    } else {
        // Non-mono, and (cp, tc, mc) != (BT709=1, SRGB=13, IDENTITY=0) —
        // that RGB special case implies 4:4:4 and is rejected by the
        // decoder for profile 0, so it must never be combined with 4:2:0.
        // The decoder then reads color_range, derives subsampling 1,1 from
        // seq_profile==0 (NO subsampling bits), reads chroma_sample_position
        // (2 bits, because subsampling_x && subsampling_y), and finally
        // separate_uv_delta_q. (libaom av1_read_color_config,
        // decodeframe.c:4210-4243; C write_color_config writes the same
        // fields for MAIN_PROFILE.)
        debug_assert!(
            !(color.color_primaries == 1
                && color.transfer_characteristics == 13
                && color.matrix_coefficients == 0),
            "sRGB-identity CICP implies 4:4:4 — invalid with profile-0 4:2:0"
        );
        debug_assert!(
            profile == 0,
            "4:2:0 color_config is only implemented for seq_profile 0 (8/10-bit)"
        );
        wb.write_bit(color.full_range); // color_range
        // seq_profile 0: subsampling_x = subsampling_y = 1 implied, no bits.
        wb.write_bits(0, 2); // chroma_sample_position = 0 (CSP_UNKNOWN)
        wb.write_bit(false); // separate_uv_delta_q = 0
    }

    wb.write_bit(false); // film_grain_params_present = 0

    write_trailing_bits(&mut wb);

    let payload = wb.into_data();
    write_obu(ObuType::SequenceHeader, &payload)
}

/// Write a key frame header for a reduced (still-picture) sequence header.
pub fn write_key_frame_header(width: u32, height: u32, base_qindex: u8) -> Vec<u8> {
    write_key_frame_header_full(
        width,
        height,
        base_qindex,
        true,
        true,
        [0; 4],
        [3, 0, 0],
        false,
    )
}

/// AV1 spec Section 5.9.2: uncompressed_header() for KEY_FRAME.
///
/// Field ordering matches the spec exactly. `monochrome` must match the
/// sequence header's mono_chrome flag: it selects NumPlanes (1 vs 3), which
/// gates the chroma delta-Q fields in quantization_params() and the chroma
/// loop-filter levels in loop_filter_params().
///
/// `lf_levels` = `[loop_filter_level[0], [1], [2] (U), [3] (V)]` per spec
/// 5.9.11; the encoder MUST apply deblocking with exactly these levels to
/// its reconstruction or the DPB diverges from every conforming decoder.
///
/// `cdef` = `[cdef_damping (3..=6), cdef_y_strength, cdef_uv_strength]` per
/// spec 5.9.19 with `cdef_bits = 0` (single strength set; the per-block
/// cdef_idx read is then ZERO arithmetic-coder bits — libaom read_cdef does
/// `aom_read_literal(r, cdef_bits)`). uv_strength is only coded for
/// NumPlanes = 3 (libaom setup_cdef, decodeframe.c:1799). Same contract as
/// deblocking: the encoder MUST apply CDEF with exactly these strengths.
///
/// `enable_restoration` MUST equal the sequence header's
/// `enable_restoration` bit: it gates lr_params() (spec 5.9.20). This
/// convenience form signals RESTORE_NONE for every plane — see
/// [`write_key_frame_header_full_lr`] for real restoration signaling.
#[allow(clippy::too_many_arguments)]
pub fn write_key_frame_header_full(
    width: u32,
    height: u32,
    base_qindex: u8,
    reduced_sh: bool,
    monochrome: bool,
    lf_levels: [u8; 4],
    cdef: [u8; 3],
    enable_restoration: bool,
) -> Vec<u8> {
    write_key_frame_header_full_lr(
        width,
        height,
        base_qindex,
        reduced_sh,
        monochrome,
        lf_levels,
        0, // loop_filter_sharpness (legacy wrapper: mainline default)
        &CdefSignal {
            damping: cdef[0],
            bits: 0,
            strengths: alloc::vec![(cdef[1], cdef[2])],
        },
        &LrSignal::none(enable_restoration),
        ScSignal::default(),
    )
}

/// Frame-header lr_params() signaling (spec 5.9.20; C
/// `encode_restoration_mode`, entropy_coding.c:2243).
#[derive(Clone, Copy, Debug)]
pub struct LrSignal {
    /// SH `enable_restoration` — gates the whole lr_params() block.
    pub enabled: bool,
    /// Per-plane RestorationType (0 NONE / 1 WIENER / 2 SGRPROJ /
    /// 3 SWITCHABLE), plane order Y, U, V.
    pub frame_types: [u8; 3],
    /// Luma restoration_unit_size (C rst_info\[0\], pcs.c:37 — 256).
    pub unit_size: u16,
    /// `lr_uv_shift` bit: chroma unit size is half luma's. C writes
    /// `rst_info[1].size != rst_info[0].size` when any chroma plane uses
    /// restoration (entropy_coding.c:2298; SVT always picks equal sizes).
    pub uv_size_differs: bool,
}

impl LrSignal {
    /// All planes RESTORE_NONE (the pre-restoration behavior).
    pub fn none(enabled: bool) -> Self {
        LrSignal {
            enabled,
            frame_types: [0; 3],
            unit_size: 256,
            uv_size_differs: false,
        }
    }
}

/// Frame-header screen-content signaling (spec 5.9.11/5.9.13; C writer
/// entropy_coding.c:3345-3359 + :3464-3466). `allow_screen_content_tools`
/// costs one bit (seq_force = SELECT) plus, when set, the
/// `force_integer_mv` bit (always 0 — resource_coordination_process.c:362)
/// and, on KEY frames at unscaled superres, the `allow_intrabc` bit. When
/// `allow_intrabc` is set the loop_filter/cdef/lr param blocks are NOT
/// coded (spec sets their defaults).
#[derive(Clone, Copy, Debug, Default)]
pub struct ScSignal {
    pub allow_screen_content_tools: bool,
    pub allow_intrabc: bool,
}

/// [`write_key_frame_header_full`] with full lr_params() signaling.
#[allow(clippy::too_many_arguments)]
/// The cdef_params() inputs (spec 5.9.19): damping 3..=6, `cdef_bits`
/// 0..=3, and the `1 << cdef_bits` packed (y, uv) strength pairs.
pub struct CdefSignal {
    pub damping: u8,
    pub bits: u8,
    pub strengths: Vec<(u8, u8)>,
}

pub fn write_key_frame_header_full_lr(
    width: u32,
    height: u32,
    base_qindex: u8,
    reduced_sh: bool,
    monochrome: bool,
    lf_levels: [u8; 4],
    lf_sharpness: u8,
    cdef: &CdefSignal,
    lr: &LrSignal,
    sc: ScSignal,
) -> Vec<u8> {
    let mut wb = key_frame_header_bits_lr(
        width,
        height,
        base_qindex,
        reduced_sh,
        monochrome,
        lf_levels,
        lf_sharpness,
        cdef,
        lr,
        sc,
    );
    // This header is embedded in an OBU_FRAME: the spec requires
    // byte_alignment() (zero bits only) between frame_header and tile_group —
    // trailing_bits (with its leading 1) is only for standalone
    // OBU_FRAME_HEADER. C: write_frame_header_obu(appendTrailingBits=0).
    byte_align_zero(&mut wb);
    wb.into_data()
}

/// Body of [`write_key_frame_header_full`] returning the raw [`BitWriter`]
/// BEFORE byte alignment (pre-alignment bit count observable for layout
/// tests — chroma delta-Q bits are zeros inside a zero run, invisible at
/// byte granularity). Bool-compat form: all planes RESTORE_NONE.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn key_frame_header_bits(
    width: u32,
    height: u32,
    base_qindex: u8,
    reduced_sh: bool,
    monochrome: bool,
    lf_levels: [u8; 4],
    cdef: [u8; 3],
    enable_restoration: bool,
) -> BitWriter {
    key_frame_header_bits_lr(
        width,
        height,
        base_qindex,
        reduced_sh,
        monochrome,
        lf_levels,
        0, // loop_filter_sharpness (test wrapper: mainline default)
        &CdefSignal {
            damping: cdef[0],
            bits: 0,
            strengths: alloc::vec![(cdef[1], cdef[2])],
        },
        &LrSignal::none(enable_restoration),
        ScSignal::default(),
    )
}

/// Body of [`write_key_frame_header_full_lr`] (see the compat wrapper above).
#[allow(clippy::too_many_arguments)]
fn key_frame_header_bits_lr(
    width: u32,
    height: u32,
    base_qindex: u8,
    reduced_sh: bool,
    monochrome: bool,
    lf_levels: [u8; 4],
    lf_sharpness: u8,
    cdef: &CdefSignal,
    lr: &LrSignal,
    sc: ScSignal,
) -> BitWriter {
    let mut wb = BitWriter::new();

    if !reduced_sh {
        // ---- Full frame header preamble ----
        wb.write_bit(false); // show_existing_frame = 0
        wb.write_bits(0, 2); // frame_type = KEY_FRAME (0)
        wb.write_bit(true); // show_frame = 1
        // showable_frame: implicit 0 for KEY_FRAME with show_frame=1
        // error_resilient_mode: implicit 1 for KEY_FRAME with show_frame=1
    }
    // For reduced SH: show_existing_frame/frame_type/show_frame/error_resilient
    // are all implicit.

    // disable_cdf_update is ALWAYS signaled (spec 5.9.2 reads it outside the
    // reduced_still_picture_header branch; C writes it unconditionally at
    // entropy_coding.c:3373).
    wb.write_bit(false); // disable_cdf_update = 0

    // allow_screen_content_tools: seq_force = SELECT → read 1 bit
    // (C entropy_coding.c:3345-3348; value from sig_deriv_multi_processes
    // _allintra :2393 = palette_level || allow_intrabc).
    wb.write_bit(sc.allow_screen_content_tools);
    if sc.allow_screen_content_tools {
        // seq_force_integer_mv = SELECT (2, sequence_control_set.c:101) →
        // the frame force_integer_mv bit follows; C keeps
        // frm_hdr->force_integer_mv = 0 unconditionally
        // (resource_coordination_process.c:362, never reassigned).
        wb.write_bit(false); // force_integer_mv = 0
    }

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

    // allow_intrabc: signaled iff allow_screen_content_tools (superres is
    // always unscaled here) — C entropy_coding.c:3464-3466, after
    // write_frame_size.
    if sc.allow_screen_content_tools {
        wb.write_bit(sc.allow_intrabc);
    }

    // ---- tile_info() ----
    write_tile_info(&mut wb, width, height);

    // ---- quantization_params() ----
    // Spec 5.9.12; decoder authority: libaom setup_quantization
    // (av1/decoder/decodeframe.c:1818-1841).
    wb.write_bits(base_qindex as u32, 8); // base_q_idx
    wb.write_bit(false); // DeltaQYDc: delta_coded = 0
    if !monochrome {
        // NumPlanes=3. The SH signaled separate_uv_delta_q=0, so the
        // decoder does NOT read diff_uv_delta (it stays 0):
        //     if (separate_uv_delta_q) diff_uv_delta = aom_rb_read_bit(rb);
        // It then reads DeltaQUDc and DeltaQUAc, and because
        // diff_uv_delta==0 the V plane reuses the U deltas — no V bits.
        // (libaom decodeframe.c:1823-1834; C write path writes the same
        // two zero delta_coded bits for u_dc/u_ac.)
        wb.write_bit(false); // DeltaQUDc: delta_coded = 0
        wb.write_bit(false); // DeltaQUAc: delta_coded = 0
    }
    // NumPlanes=1 (mono_chrome): no DeltaQUDc, DeltaQUAc
    wb.write_bit(false); // using_qmatrix = 0

    // ---- segmentation_params() ----
    wb.write_bit(false); // segmentation_enabled = 0

    // ---- delta_q_params() ----
    // Spec: delta_q_present is only signaled when base_q_idx > 0.
    if base_qindex > 0 {
        wb.write_bit(false); // delta_q_present = 0
    }
    // delta_lf_params(): not signaled when delta_q_present=0

    // ---- loop_filter_params() ----
    // CodedLossless is only true when base_q_idx=0 AND all delta-Q=0 AND
    // all segments have qindex 0. With base_q_idx>0 in practice, not lossless.
    // When allow_intrabc: NO loop filter bits (spec 5.9.11 sets defaults).
    // Field set matches C encode_loopfilter (entropy_coding.c:2338) and
    // spec 5.9.11; libaom setup_loopfilter (decodeframe.c:1766) reads it.
    if !sc.allow_intrabc {
        wb.write_bits(lf_levels[0] as u32, 6); // loop_filter_level[0]
        wb.write_bits(lf_levels[1] as u32, 6); // loop_filter_level[1]
        // NumPlanes=1: no loop_filter_level[2]/[3].
        // NumPlanes=3: levels [2] (U) and [3] (V) are only coded when
        // (loop_filter_level[0] || loop_filter_level[1]).
        if !monochrome && (lf_levels[0] != 0 || lf_levels[1] != 0) {
            wb.write_bits(lf_levels[2] as u32, 6); // loop_filter_level[2] (U)
            wb.write_bits(lf_levels[3] as u32, 6); // loop_filter_level[3] (V)
        }
        wb.write_bits(u32::from(lf_sharpness), 3); // loop_filter_sharpness (fork default 1)
        // loop_filter_delta_enabled = 0: the C encoder runs with
        // mode_ref_delta_enabled = 0 (resource_coordination_process.c:393) and
        // encode_loopfilter writes the flag verbatim, so no ref/mode deltas are
        // signaled or applied — the filter level is uniform per plane/direction.
        wb.write_bit(false); // loop_filter_delta_enabled = 0
    }

    // ---- cdef_params() ----
    // Spec 5.9.19; C write path encode_cdef (entropy_coding.c:2398), read
    // path libaom setup_cdef (decodeframe.c:1799). Present because the SH
    // signals enable_cdef=1 and this header is neither CodedLossless
    // (base_q_idx > 0 in practice — same standing assumption as
    // loop_filter_params above) nor allow_intrabc.
    // When allow_intrabc: NO cdef bits (spec 5.9.19 early-out).
    if !sc.allow_intrabc {
        debug_assert!((3..=6).contains(&cdef.damping), "cdef_damping out of range");
        debug_assert_eq!(cdef.strengths.len(), 1usize << cdef.bits);
        wb.write_bits((cdef.damping - 3) as u32, 2); // cdef_damping_minus_3
        wb.write_bits(cdef.bits as u32, 2); // cdef_bits -> (1 << bits) strength sets
        for &(y, uv) in &cdef.strengths {
            wb.write_bits(y as u32, 6); // cdef_y_pri(4) + cdef_y_sec(2) packed
            if !monochrome {
                // NumPlanes=3 only: libaom reads uv strengths iff num_planes > 1
                // (C SVT always writes both — it cannot emit monochrome).
                wb.write_bits(uv as u32, 6); // cdef_uv_pri(4) + cdef_uv_sec(2)
            }
        }
    }

    // ---- lr_params() ----
    // Spec 5.9.20; C writer: encode_restoration_mode
    // (entropy_coding.c:2243-2307), whose call is gated on
    // seq_header.enable_restoration at entropy_coding.c:3652-3653 (the
    // spec folds the gate into lr_params' AllLossless/allow_intrabc/
    // enable_restoration early-out; base_q_idx > 0 and
    // allow_intrabc = 0 are the same standing assumptions as
    // cdef_params above). Per-plane bit pairs (entropy_coding.c:2263-2282;
    // read as lr_type f(2) with Remap_Lr_Type): NONE (0,0), WIENER (1,0),
    // SGRPROJ (1,1), SWITCHABLE (0,1). When any plane restores, the luma
    // unit-size bits follow (sb 64: bit(size>64), then bit(size>128));
    // when a CHROMA plane restores, the lr_uv_shift bit follows.
    // NumPlanes = 1 for mono, 3 for 4:2:0 (C is always 3-plane; the
    // decoder reads NumPlanes lr_types — libaom decode_restoration_mode,
    // decodeframe.c).
    // When allow_intrabc: NO lr bits (spec 5.9.20 folds allow_intrabc into
    // the same early-out as !enable_restoration).
    if lr.enabled && !sc.allow_intrabc {
        let num_planes = if monochrome { 1 } else { 3 };
        let mut all_none = true;
        let mut chroma_none = true;
        for p in 0..num_planes {
            let t = lr.frame_types[p];
            if t != 0 {
                all_none = false;
                if p > 0 {
                    chroma_none = false;
                }
            }
            let (b0, b1) = match t {
                0 => (false, false), // RESTORE_NONE
                1 => (true, false),  // RESTORE_WIENER
                2 => (true, true),   // RESTORE_SGRPROJ
                _ => (false, true),  // RESTORE_SWITCHABLE
            };
            wb.write_bit(b0);
            wb.write_bit(b1);
        }
        if !all_none {
            // sb_size is 64 in this encoder (C asserts unit >= sb).
            debug_assert!(lr.unit_size >= 64);
            wb.write_bit(lr.unit_size > 64);
            if lr.unit_size > 64 {
                wb.write_bit(lr.unit_size > 128);
            }
        }
        if !chroma_none {
            wb.write_bit(lr.uv_size_differs);
        }
    }

    // ---- read_tx_mode() ----
    // Not CodedLossless (since base_q_idx may be nonzero) →
    // TX_MODE_SELECT, like C: SVT always sets frm_hdr->tx_mode =
    // TX_MODE_SELECT at these presets ("Use TX_MODE_SELECT even when
    // txs_level == 0", enc_mode_config.c:15140-15143; written at
    // entropy_coding.c:3659). The tile walk then codes a per-block
    // tx_depth symbol for every bsize > 4x4 (always depth 0 = largest —
    // matching what the LARGEST mode implied, but now in C's syntax).
    wb.write_bit(true); // tx_mode_select = 1 → TX_MODE_SELECT

    // For intra frames: no reference_select, skip_mode, warped_motion, global_motion

    wb.write_bit(false); // reduced_tx_set = 0

    // NOTE: byte_alignment() is applied by the caller
    // (write_key_frame_header_full) so tests can observe the raw bit count.
    wb
}

/// byte_alignment(): pad with zero bits to the next byte boundary.
fn byte_align_zero(wb: &mut BitWriter) {
    let remainder = wb.bit_offset % 8;
    if remainder != 0 {
        wb.write_bits(0, 8 - remainder);
    }
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
    // The decoder reads increment_tile_cols_log2 bits until a 0: a single
    // 0 keeps TileColsLog2 = 0 (single tile column). No bit at all when
    // no increment is even possible (maxLog2TileCols == 0).
    if max_log2_tile_cols > 0 {
        wb.write_bit(false); // increment_tile_cols_log2 = 0 → stop
    }

    // TileRowsLog2 starts at max(minLog2Tiles - TileColsLog2, 0) = 0
    // maxLog2TileRows = tile_log2(min(sbRows, MAX_TILE_ROWS))
    // MAX_TILE_ROWS = 64 in AV1 spec
    let max_log2_tile_rows = tile_log2(sb_rows.min(64));
    if max_log2_tile_rows > 0 {
        wb.write_bit(false); // increment_tile_rows_log2 = 0 → stop
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

    // ---- cdef_params() ---- (SH signals enable_cdef=1, so every
    // non-lossless FH carries them). Inter frames don't run CDEF yet:
    // zero strengths keep the decoder's do_cdef gate false — signaling
    // and (non-)application stay consistent. Damping uses the same
    // C derivation as key frames (CDEF_DAMPING_FROM_QP, enc_cdef.c:923)
    // so the field is always legal and uniform across frame types.
    wb.write_bits((base_qindex >> 6) as u32, 2); // cdef_damping_minus_3
    wb.write_bits(0, 2); // cdef_bits = 0
    wb.write_bits(0, 6); // cdef_y_strength[0] = 0 (mono: no uv field)
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
    // Spec 5.11.1: tile_start_and_end_present_flag is only read when
    // NumTiles > 1. For a single tile the tile group has NO header bits —
    // the (already byte-aligned) tile data starts immediately. C reference:
    // write_tile_group_header returns 0 bytes when tiles_log2 == 0.
    tile_data.to_vec()
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
    // NumTiles > 1: tile_start_and_end_present_flag = 0 (all tiles in this
    // TG), then byte_alignment() — zero bits, NOT trailing_bits (spec 5.11.1).
    wb.write_bit(false);
    byte_align_zero(&mut wb);
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
    use alloc::vec;

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
        assert_eq!(
            write_sequence_header_full(64, 64),
            [
                0x0a, 0x0d, 0x00, 0x00, 0x00, 0x02, 0xaf, 0xff, 0x80, 0x4d, 0xa6, 0x02, 0x1a, 0x02,
                0x40
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
        let ours =
            write_sequence_header_ex(
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
                enable_filter_intra: true,
                enable_intra_edge_filter: false,
                enable_restoration: true,
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
        let bits_420 = key_frame_header_bits(64, 64, 30, true, false, [0; 4], [3, 0, 0], false).bit_offset;
        let bits_mono = key_frame_header_bits(64, 64, 30, true, true, [0; 4], [3, 0, 0], false).bit_offset;
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
        let b420 = key_frame_header_bits(64, 64, 220, true, false, [0; 4], [6, 43, 7], false).bit_offset;
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
}
