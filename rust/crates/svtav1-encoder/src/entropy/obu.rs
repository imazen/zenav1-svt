//! OBU (Open Bitstream Unit) writer for AV1 bitstreams.
//!
//! Spec 07 §5.3: OBU bitstream format.
//!
//! Produces valid AV1 bitstream output. All field orderings match
//! the AV1 specification (av1-spec-errata1) exactly.

use alloc::vec::Vec;
use svtav1_types::bitstream::{MAX_TILE_AREA, MAX_TILE_COLS, MAX_TILE_ROWS, MAX_TILE_WIDTH};
use svtav1_types::restoration::MAX_SEGMENTS;
use svtav1_types::segmentation::{
    SEG_LVL_MAX, SEGMENTATION_FEATURE_BITS, SEGMENTATION_FEATURE_SIGNED, SegmentationParams,
};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqTools {
    /// SH `separate_uv_delta_q` (spec 5.5.2 color_config): the svt-av1-hdr
    /// fork ALWAYS signals 1 (its chroma-qindex path writes distinct U/V
    /// deltas); mainline derives it from the chroma offsets (0 in this
    /// port's envelope). When true, the FH may carry per-plane deltas.
    pub separate_uv_delta_q: bool,
    /// SH `film_grain_params_present` (spec 5.5.1): the fork's photon
    /// noise (`--noise*`) signals a synthesized grain table per frame.
    pub film_grain_params_present: bool,
    /// SH `enable_filter_intra` (spec 5.5.1): gates the per-block
    /// `use_filter_intra` symbol for eligible intra blocks
    /// ([`crate::entropy::context::write_use_filter_intra`]).
    pub enable_filter_intra: bool,
    /// SH `enable_intra_edge_filter` (spec 5.5.1): the DECODER then
    /// filters/upsamples directional-prediction edges
    /// (libaom build_intra_predictors `disable_edge_filter`), so the
    /// encoder's reconstruction path must do the same. C allintra
    /// derivation (`svt_aom_sig_deriv_pre_analysis_scs`,
    /// enc_mode_config.c:4036-4048): 1 iff
    /// `dist_based_ang_intra_level >= 1 || angular_pred_level[intra_level]
    /// in {2, 3}` — true ONLY at M5 (intra_level 2 -> angular_pred_level 2)
    /// among representable allintra presets.
    pub enable_intra_edge_filter: bool,
    /// SH `enable_restoration` (spec 5.5.1): gates the FH lr_params()
    /// fields (spec 5.9.20) — every frame header of the sequence must
    /// then carry per-plane `lr_type` bits.
    pub enable_restoration: bool,
    /// SH `use_128x128_superblock` (spec 5.5.1). C never stores this — it
    /// derives it at write time from `seq_header.sb_size == BLOCK_128X128`
    /// (entropy_coding.c:2800); the struct field at :190 is dead. The port
    /// mirrors that by deriving it from `EncodePipeline::sb_size`, which in
    /// turn comes from `sb128_geom::derive_super_block_size` (the C rule at
    /// Globals/enc_handle.c:4071-4111).
    ///
    /// Default false = 64px superblocks, which is what the C rule returns
    /// for every cell the gates currently cover.
    /// The bool <-> pixels mapping deliberately lives in ONE place —
    /// `sb128_geom::sb_header_params`, which also returns `sb_mi_size` and
    /// BOTH log2 domains (MI and PIXEL, which the port map flags as easy to
    /// conflate). Do not add a second accessor here.
    pub use_128x128_superblock: bool,
    /// SH `enable_superres` (spec 5.5.1): gates the FH `superres_params()`
    /// `use_superres` bit (spec 5.9.8). C sets it from the superres config
    /// (`--superres-mode != 0`); MEASURED against real C v4.2.0 output at
    /// `--superres-mode 1 --superres-kf-denom {12,16}` on a 64x64 still.
    ///
    /// Default false = no bit change anywhere, which is what every existing
    /// byte-identical gate cell encodes.
    pub enable_superres: bool,
    /// SH `chroma_sample_position` (spec 5.5.2, 2 bits, written only for
    /// 4:2:0 with `mono_chrome = 0`): 0 = CSP_UNKNOWN, 1 = CSP_VERTICAL,
    /// 2 = CSP_COLOCATED. C `static_config.chroma_sample_position`
    /// (entropy_coding.c:2743), default `EB_CSP_UNKNOWN` — every
    /// pre-existing caller writes the same 0 bits.
    pub chroma_sample_position: u8,
    /// SH `enable_interintra_compound`, `enable_masked_compound`,
    /// `enable_jnt_comp`, `enable_warped_motion`, `enable_ref_frame_mvs`,
    /// `enable_order_hint`, `enable_dual_filter` (spec 5.5.1) — the INTER
    /// tool bits. The reduced (still) header writes NONE of them, so their
    /// value is inert on every still cell; the video header derives them in
    /// `speed_config::seq_tools_video` from C's
    /// `svt_aom_sig_deriv_pre_analysis_scs` (enc_mode_config.c:2780).
    pub enable_interintra_compound: bool,
    /// See [`Self::enable_interintra_compound`].
    pub enable_masked_compound: bool,
    /// See [`Self::enable_interintra_compound`].
    pub enable_jnt_comp: bool,
    /// See [`Self::enable_interintra_compound`].
    pub enable_warped_motion: bool,
    /// See [`Self::enable_interintra_compound`]. Gates the FH
    /// `use_ref_frame_mvs` field, which spec 5.9.2 writes only for a
    /// non-intra frame.
    pub enable_ref_frame_mvs: bool,
    /// See [`Self::enable_interintra_compound`]. When 0 the SH also omits
    /// `enable_jnt_comp`, `enable_ref_frame_mvs` and
    /// `order_hint_bits_minus_1`.
    pub enable_order_hint: bool,
    /// See [`Self::enable_interintra_compound`].
    pub enable_dual_filter: bool,
    /// The config's `hierarchical_levels`, used ONLY to derive the
    /// non-reduced header's `initial_display_delay`
    /// (`min(hierarchical_levels + 1, 10)`, enc_handle.c:4975-4993). It has
    /// no effect on a still (reduced) header, where those bits are not
    /// written at all — so the default 0 keeps every existing cell
    /// byte-identical.
    pub hierarchical_levels: u8,
    /// The coded chroma subsampling format (spec 5.5.2): selects
    /// `seq_profile`, the subsampling bits (profiles 1/2), and whether
    /// `chroma_sample_position` is written at all. Default `Yuv420` —
    /// profile 0, subsampling implied 1,1 — byte-identical to every
    /// existing cell. Non-420 formats are Zen extensions (C refuses
    /// them); see `svtav1_types::chroma::ChromaFormat`.
    pub chroma_format: svtav1_types::chroma::ChromaFormat,
}

impl Default for SeqTools {
    /// Every tool bit off, EXCEPT `enable_order_hint`.
    ///
    /// Order hints are the one inter field C never disables (it is 1 in every
    /// non-reduced header SVT-AV1 writes), and the two convenience wrappers
    /// that take this default — [`write_sequence_header`] (reduced, where no
    /// inter bit is written at all) and [`write_sequence_header_full`] —
    /// have always emitted `enable_order_hint = 1`. A `#[derive(Default)]`
    /// would silently flip that to 0 and, with it, drop `enable_jnt_comp`,
    /// `enable_ref_frame_mvs` and `order_hint_bits_minus_1` from the full
    /// header — a header shape C never emits. Hence the manual impl.
    fn default() -> Self {
        Self {
            separate_uv_delta_q: false,
            film_grain_params_present: false,
            enable_filter_intra: false,
            enable_intra_edge_filter: false,
            enable_restoration: false,
            use_128x128_superblock: false,
            enable_superres: false,
            chroma_sample_position: 0,
            enable_interintra_compound: false,
            enable_masked_compound: false,
            enable_jnt_comp: false,
            enable_warped_motion: false,
            enable_ref_frame_mvs: false,
            enable_order_hint: true,
            enable_dual_filter: false,
            hierarchical_levels: 0,
            chroma_format: svtav1_types::chroma::ChromaFormat::Yuv420,
        }
    }
}

/// FH `superres_params()` (spec 5.9.8) — the frame's superres state.
///
/// `None` = `use_superres = 0` (the default; `SuperresDenom = SCALE_NUMERATOR
/// = 8`, coded frame width == upscaled width). `Some(denom)` with `denom` in
/// 9..=16 codes `use_superres = 1` + `coded_denom = denom - 9` (3 bits) and
/// means the frame is CODED at `upscaled_w * 8 / denom` and upscaled back by
/// the decoder ([`svtav1_dsp::superres`]).
///
/// MEASURED ground truth (real C v4.2.0, 64x64 still, `--superres-mode 1`):
/// `--superres-kf-denom 12` -> `use_superres = 1, coded_denom = 3`;
/// `--superres-kf-denom 16` -> `coded_denom = 7`. NOTE for a still/KEY frame
/// the applicable C knob is `--superres-kf-denom`; `--superres-denom` alone
/// leaves `use_superres = 0` on the key frame (also measured).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SuperresParams {
    /// The SEQUENCE header's `enable_superres` bit ([`SeqTools::
    /// enable_superres`]), repeated here because `superres_params()` codes
    /// nothing at all when the sequence has the tool off — and the two must
    /// agree or the decoder's bit walk desyncs. C can have this set while a
    /// given frame still codes `use_superres = 0` (MEASURED: `--superres-mode
    /// 1 --superres-denom 12` on a still leaves the KEY frame unscaled).
    pub enabled_in_seq: bool,
    /// `SuperresDenom` in 9..=16, or `None` for unscaled (denom 8).
    pub denom: Option<u8>,
}

impl SuperresParams {
    /// Spec 5.9.8 `coded_denom` (`SuperresDenom - SUPERRES_DENOM_MIN`).
    pub const DENOM_MIN: u8 = 9;
    /// Spec `SUPERRES_NUM` — the unscaled denominator.
    pub const NUM: u8 = 8;

    /// Write `superres_params()` for a frame whose sequence header signalled
    /// `enable_superres = enable`. Nothing is coded when the sequence has the
    /// tool off, exactly like C.
    fn write(self, wb: &mut BitWriter) {
        if !self.enabled_in_seq {
            debug_assert!(
                self.denom.is_none(),
                "a frame cannot use superres unless the SH enables it"
            );
            return;
        }
        match self.denom {
            Some(d) => {
                debug_assert!(
                    (Self::DENOM_MIN..=16).contains(&d),
                    "SuperresDenom must be 9..=16, got {d}"
                );
                wb.write_bit(true); // use_superres = 1
                wb.write_bits(u32::from(d - Self::DENOM_MIN), 3); // coded_denom
            }
            None => wb.write_bit(false), // use_superres = 0
        }
    }

    /// Spec 5.9.8 `FrameWidth` — the CODED width for an upscaled width.
    /// Identical to C `calculate_scaled_size_helper`'s formula for the
    /// superres denominators (both are `(w * 8 + denom/2) / denom`).
    pub fn coded_width(self, upscaled_width: u32) -> u32 {
        match self.denom {
            Some(d) => (upscaled_width * u32::from(Self::NUM) + u32::from(d) / 2) / u32::from(d),
            None => upscaled_width,
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

    /// Number of BITS written so far. `data()` zero-pads the final byte, so
    /// a caller that needs the exact payload length (a differential bit-layout
    /// check, say) must read this rather than `bytes_written() * 8`.
    pub fn bit_len(&self) -> usize {
        self.bit_offset as usize
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

/// A `show_existing_frame` packet (spec 5.9.2; C `entropy_coding.c:3308-3334`
/// + `:3849-3907`) — the OBU_FRAME_HEADER a random-access stream emits at a
/// hidden picture's display position.
///
/// C writes ONLY `show_existing_frame = 1` and the 3-bit
/// `frame_to_show_map_idx` (the DPB slot to re-display), then
/// `add_trailing_bits`: the payload is the single byte `1 sss 1 000`
/// (`0x88 | slot << 3`). Verified byte-for-byte against the C driver at
/// `SVT_PRED_STRUCT=2` — a capture of `SvtAv1EncApp`'s HL3 stream shows
/// `d8`/`c8`/`98` payloads = slots 5/4/1.
///
/// `tiles_log2` is the frame's `log2_tile_rows + log2_tile_cols`: C still
/// appends the tile-group header to the OBU (`write_tile_group_header`),
/// which is a zero-length field only when `tiles_log2 == 0` — a multi-tile
/// encode needs the extra `tile_start_and_end_present_flag = 0` byte.
#[must_use]
pub fn write_show_existing_obu(frame_to_show_map_idx: u8, tiles_log2: u8) -> Vec<u8> {
    debug_assert!(
        frame_to_show_map_idx < 8,
        "show_existing_frame names a DPB slot, 0..=7"
    );
    let mut wb = BitWriter::new();
    wb.write_bit(true); // show_existing_frame = 1
    wb.write_bits(u32::from(frame_to_show_map_idx), 3);
    // add_trailing_bits: a 1 bit, then zero padding to the byte edge — the
    // same `1 sss 1 000` byte the C capture shows.
    wb.write_bit(true);
    byte_align_zero(&mut wb);
    if tiles_log2 > 0 {
        wb.write_bit(false); // tile_start_and_end_present_flag = 0
        byte_align_zero(&mut wb);
    }
    write_obu(ObuType::FrameHeader, &wb.into_data())
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

/// Canonical AV1 film-grain parameters (spec 5.9.30), shared by C noise
/// estimation, supplied tables, photon noise, signaling and reconstruction.
/// INTER update/ref syntax is carried by [`InterSignal`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilmGrainParams {
    pub apply_grain: bool,
    /// 16-bit seed; C starts at 7391 and adds 3381 per frame
    /// (resource_coordination_process.c:314, sequence_control_set.c:76).
    pub random_seed: u16,
    pub num_y_points: usize,
    pub scaling_points_y: [[u8; 2]; 14],
    pub chroma_scaling_from_luma: bool,
    pub num_cb_points: usize,
    pub scaling_points_cb: [[u8; 2]; 10],
    pub num_cr_points: usize,
    pub scaling_points_cr: [[u8; 2]; 10],
    /// Signaled as `scaling_shift - 8` (2 bits).
    pub scaling_shift: u8,
    pub ar_coeff_lag: u8,
    pub ar_coeffs_y: [i16; 24],
    pub ar_coeffs_cb: [i16; 25],
    pub ar_coeffs_cr: [i16; 25],
    /// Signaled as `ar_coeff_shift - 6` (2 bits).
    pub ar_coeff_shift: u8,
    pub grain_scale_shift: u8,
    pub cb_mult: u8,
    pub cb_luma_mult: u8,
    pub cb_offset: u16,
    pub cr_mult: u8,
    pub cr_luma_mult: u8,
    pub cr_offset: u16,
    pub overlap_flag: bool,
    pub clip_to_restricted_range: bool,
}

/// Write C KEY/INTER `film_grain_params` (entropy_coding.c:3105).
/// KEY updates implicitly; INTER either updates or reuses a reference table.
/// 4:2:0 non-mono form: chroma points are written unless
/// `num_y_points == 0` forces them off (the subsampling-1,1 rule) or
/// chroma_scaling_from_luma is set.
fn write_film_grain_params(wb: &mut BitWriter, fg: &FilmGrainParams, inter: Option<&InterSignal>) {
    wb.write_bit(fg.apply_grain);
    if !fg.apply_grain {
        return;
    }
    wb.write_bits(u32::from(fg.random_seed), 16);
    if let Some(signal) = inter {
        wb.write_bit(signal.film_grain_ref_idx.is_none());
        if let Some(slot) = signal.film_grain_ref_idx {
            assert!(slot < 8 && signal.ref_frame_idx.contains(&slot));
            wb.write_bits(u32::from(slot), 3);
            return;
        }
    }
    wb.write_bits(fg.num_y_points as u32, 4);
    for p in &fg.scaling_points_y[..fg.num_y_points] {
        wb.write_bits(u32::from(p[0]), 8);
        wb.write_bits(u32::from(p[1]), 8);
    }
    wb.write_bit(fg.chroma_scaling_from_luma);
    let chroma_off = fg.chroma_scaling_from_luma || fg.num_y_points == 0;
    let (num_cb, num_cr) = if chroma_off {
        (0, 0)
    } else {
        (fg.num_cb_points, fg.num_cr_points)
    };
    if !chroma_off {
        wb.write_bits(num_cb as u32, 4);
        for p in &fg.scaling_points_cb[..num_cb] {
            wb.write_bits(u32::from(p[0]), 8);
            wb.write_bits(u32::from(p[1]), 8);
        }
        wb.write_bits(num_cr as u32, 4);
        for p in &fg.scaling_points_cr[..num_cr] {
            wb.write_bits(u32::from(p[0]), 8);
            wb.write_bits(u32::from(p[1]), 8);
        }
    }
    wb.write_bits(u32::from(fg.scaling_shift - 8), 2);
    wb.write_bits(u32::from(fg.ar_coeff_lag), 2);
    let num_pos_luma = 2 * usize::from(fg.ar_coeff_lag) * (usize::from(fg.ar_coeff_lag) + 1);
    let num_pos_chroma = num_pos_luma + usize::from(fg.num_y_points > 0);
    if fg.num_y_points > 0 {
        for &c in &fg.ar_coeffs_y[..num_pos_luma] {
            wb.write_bits((c + 128) as u32, 8);
        }
    }
    if num_cb > 0 || fg.chroma_scaling_from_luma {
        for &c in &fg.ar_coeffs_cb[..num_pos_chroma] {
            wb.write_bits((c + 128) as u32, 8);
        }
    }
    if num_cr > 0 || fg.chroma_scaling_from_luma {
        for &c in &fg.ar_coeffs_cr[..num_pos_chroma] {
            wb.write_bits((c + 128) as u32, 8);
        }
    }
    wb.write_bits(u32::from(fg.ar_coeff_shift - 6), 2);
    wb.write_bits(u32::from(fg.grain_scale_shift), 2);
    if num_cb > 0 {
        wb.write_bits(u32::from(fg.cb_mult), 8);
        wb.write_bits(u32::from(fg.cb_luma_mult), 8);
        wb.write_bits(u32::from(fg.cb_offset), 9);
    }
    if num_cr > 0 {
        wb.write_bits(u32::from(fg.cr_mult), 8);
        wb.write_bits(u32::from(fg.cr_luma_mult), 8);
        wb.write_bits(u32::from(fg.cr_offset), 9);
    }
    wb.write_bit(fg.overlap_flag);
    wb.write_bit(fg.clip_to_restricted_range);
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

    // seq_profile: mono -> 0 (2 at 12-bit); chroma -> the profile the
    // format requires (spec 6.4.1: 444 -> 1 at <=10-bit, 2 at 12-bit;
    // 422 -> 2; 420 -> 0). C `verify_settings` couples them identically
    // (enc_settings.c:475-490).
    let profile = if monochrome {
        if bit_depth > 10 { 2 } else { 0 }
    } else {
        tools.chroma_format.required_profile(bit_depth)
    };
    wb.write_bits(u32::from(profile), 3);
    wb.write_bit(still_picture);
    wb.write_bit(still_picture); // reduced_still_picture_header = still_picture

    // C-exact auto level (set_bitstream_level_tier; 64x64@30 → 2.0 → 0).
    let seq_level_idx = compute_seq_level_idx(width, height, fps);

    if still_picture {
        // Reduced header: only seq_level_idx
        wb.write_bits(seq_level_idx as u32, 5);
    } else {
        wb.write_bit(false); // timing_info_present_flag = 0
        // initial_display_delay_present_flag: C sets this to 1 for every
        // non-reduced header (enc_handle.c:4990), NOT 0. Writing 0 here
        // omitted the five bits C always emits for operating point 0 (the
        // per-op present bit plus the 4-bit delay), which shifted every
        // following field — one of the four defects the inter refusal in
        // pipeline.rs names.
        //
        // The delay itself is `min(hierarchical_levels + 1, 10)` — "the number
        // of decoded frames that should be present in the buffer pool before
        // the first presentable frame is displayed" (spec 6.4.1), which for
        // SVT's TU output is one frame per temporal layer plus the displayable
        // leaf (enc_handle.c:4975-4993). It is written biased by one.
        wb.write_bit(true); // initial_display_delay_present_flag = 1
        wb.write_bits(0, 5); // operating_points_cnt_minus_1 = 0
        wb.write_bits(0, 12); // operating_point_idc[0] = 0
        wb.write_bits(seq_level_idx as u32, 5); // seq_level_idx[0]
        // seq_tier is only coded for seq_level_idx > 7 (level major > 3):
        // spec 5.5.1 and C write_sequence_header_obu
        // (entropy_coding.c:3790-3792, `if (scs->level[i].major > 3)`).
        if seq_level_idx > 7 {
            wb.write_bit(false); // seq_tier[0] = 0 (main tier)
        }
        // entropy_coding.c:3749-3755, inside the operating-point loop.
        wb.write_bit(true); // initial_display_delay_present_for_this_op = 1
        let display_delay = u32::from(tools.hierarchical_levels)
            .saturating_add(1)
            .min(10);
        wb.write_bits(display_delay - 1, 4); // initial_display_delay_minus_1
    }

    // Frame dimensions.
    //
    // `.max(1)` IS LOAD-BEARING, and its absence was a shipping defect at
    // width or height 1. `32 - (1 - 1).leading_zeros()` is 0, so `w_bits - 1`
    // below underflowed: a release build wrote `frame_width_bits_minus_1` as
    // the low 4 bits of `u32::MAX` (15, i.e. "16 bits follow") and then wrote
    // ZERO bits of `max_frame_width_minus_1`, so every following field was
    // read at the wrong offset. MEASURED 2026-09-03: the port's 1x1 stream is
    // 21 bytes and dav1d says "Error parsing sequence header / Overrun in OBU
    // bit buffer"; C's 1x1 stream is also 21 bytes and decodes. 8x1 and 1x8
    // fail the same way, on the 4:2:0 path as well as the monochrome one.
    //
    // Spec 5.5.1 reads `max_frame_width_minus_1` as `f(n)` with
    // `n = frame_width_bits_minus_1 + 1`, so n is at least 1 and the minimum
    // legal encoding of width 1 is `frame_width_bits_minus_1 = 0` plus one
    // zero bit. C is not in fact a second transcription risk here: it derives
    // the count from `svt_aom_get_msb`, which is only ever called on a
    // non-zero value in its own guarded path.
    //
    // This is C-ENVELOPE work, not out-of-envelope generosity:
    // `svt_av1_verify_settings` accepts width and height down to 1
    // ("Source Width must be at least 1", enc_settings.c:50), so a byte oracle
    // exists at these sizes.
    let w_bits = (32 - (width - 1).leading_zeros()).max(1);
    let h_bits = (32 - (height - 1).leading_zeros()).max(1);
    wb.write_bits(w_bits - 1, 4); // frame_width_bits_minus_1
    wb.write_bits(h_bits - 1, 4); // frame_height_bits_minus_1
    wb.write_bits(width - 1, w_bits); // max_frame_width_minus_1
    wb.write_bits(height - 1, h_bits); // max_frame_height_minus_1

    if !still_picture {
        wb.write_bit(false); // frame_id_numbers_present_flag = 0
    }

    // C derives this at write time from `sb_size == BLOCK_128X128`
    // (entropy_coding.c:2800) — see `SeqTools::use_128x128_superblock`.
    wb.write_bit(tools.use_128x128_superblock);
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
        wb.write_bit(tools.enable_interintra_compound);
        wb.write_bit(tools.enable_masked_compound);
        wb.write_bit(tools.enable_warped_motion);
        wb.write_bit(tools.enable_dual_filter);
        wb.write_bit(tools.enable_order_hint);
        // Spec 5.5.1 and C (entropy_coding.c:2814-2817): these two are coded
        // only when order hints are enabled.
        if tools.enable_order_hint {
            wb.write_bit(tools.enable_jnt_comp);
            wb.write_bit(tools.enable_ref_frame_mvs);
        }

        // seq_choose_screen_content_tools (1 bit, NOT 2!)
        wb.write_bit(true); // = 1 → seq_force_screen_content_tools = SELECT

        // seq_force_screen_content_tools > 0 (SELECT=2 > 0), so:
        // seq_choose_integer_mv (1 bit)
        wb.write_bit(true); // = 1 → seq_force_integer_mv = SELECT

        // order_hint_bits_minus_1 comes AFTER the screen-content and
        // integer-mv bits, not before them: spec 5.5.1 and C
        // (entropy_coding.c:2836-2838, the second
        // `if (enable_order_hint)` block, which follows the two
        // seq_choose_* blocks). Writing it early put three bits in the wrong
        // place and shifted everything after — the second of the four defects
        // the inter refusal in pipeline.rs names.
        // Also conditioned on enable_order_hint (entropy_coding.c:2836).
        if tools.enable_order_hint {
            wb.write_bits(ORDER_HINT_BITS - 1, 3); // order_hint_bits_minus_1
        }
    }

    // enable_superres (spec 5.5.1): false on every existing gate cell, so the
    // written bit is unchanged there. When true the frame header carries
    // `superres_params()` (spec 5.9.8) — see [`SuperresParams`].
    wb.write_bit(tools.enable_superres);
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
    // (spec 5.5.2: written iff seq_profile != 1 — profile 1 implies 0.
    // C entropy_coding.c:2690-2694.)
    if profile != 1 {
        wb.write_bit(monochrome);
    }

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
    } else if color.color_primaries == 1
        && color.transfer_characteristics == 13
        && color.matrix_coefficients == 0
    {
        // sRGB-identity CICP (spec 5.5.2): implies 4:4:4 — the decoder
        // skips color_range AND the subsampling bits entirely and derives
        // subsampling_x = subsampling_y = 0. Requires profile 1 (or
        // profile 2 at 12-bit); C asserts ss 0,0
        // (entropy_coding.c:2736-2741).
        debug_assert!(
            profile != 0 && tools.chroma_format.subsampling_x() == 0,
            "sRGB-identity CICP requires 4:4:4 subsampling (profile 1, or 2 at 12-bit)"
        );
    } else {
        // Non-mono, non-identity CICP: color_range, then subsampling per
        // profile (C write_color_config, entropy_coding.c:2712-2744):
        //   profile 0 -> 4:2:0 implied (no bits)
        //   profile 1 -> 4:4:4 implied (no bits)
        //   profile 2 -> 12-bit: subsampling_x bit, then subsampling_y
        //     iff x==1; <=10-bit -> 4:2:2 implied (no bits).
        wb.write_bit(color.full_range); // color_range
        let (ss_x, ss_y) = (
            tools.chroma_format.subsampling_x(),
            tools.chroma_format.subsampling_y(),
        );
        match profile {
            0 => debug_assert!(ss_x == 1 && ss_y == 1, "profile 0 is 4:2:0 only"),
            1 => debug_assert!(ss_x == 0 && ss_y == 0, "profile 1 is 4:4:4 only"),
            _ => {
                if bit_depth == 12 {
                    wb.write_bit(ss_x == 1); // subsampling_x
                    if ss_x == 1 {
                        wb.write_bit(ss_y == 1); // subsampling_y
                    }
                    debug_assert!(
                        !(ss_x == 0 && ss_y == 1),
                        "4:4:0 subsampling not allowed in AV1"
                    );
                } else {
                    debug_assert!(
                        ss_x == 1 && ss_y == 0,
                        "profile 2 at <=10-bit is 4:2:2 only"
                    );
                }
            }
        }
        // chroma_sample_position: 2 bits iff subsampling_x && subsampling_y
        // (C entropy_coding.c:2743).
        if ss_x == 1 && ss_y == 1 {
            wb.write_bits(u32::from(tools.chroma_sample_position & 3), 2);
        }
        wb.write_bit(tools.separate_uv_delta_q); // separate_uv_delta_q (fork: 1)
    }

    wb.write_bit(tools.film_grain_params_present); // film_grain_params_present

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
        None, // chroma_q: mainline zero-delta bit pattern
        None, // delta_q_res: no per-SB delta-q
        None, // qm: quant matrices off
        None, // fgs: no film grain
        0,    // tile_rows_log2: legacy wrapper stays single-tile
        0,    // tile_cols_log2: ditto
        0,    // tile_size_bytes_minus_1: unused when NumTiles == 1
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
    /// FH `superres_params()` (spec 5.9.8) — see [`SuperresParams`]. Default
    /// (`denom: None`) is the unscaled frame every current gate cell encodes;
    /// it also gates `allow_intrabc` off, matching the spec's
    /// `UpscaledWidth == FrameWidth` condition.
    pub superres: SuperresParams,
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

/// `tile_rows_log2` MUST already be resolved via
/// [`resolve_tile_rows_log2`] (the value the caller actually split the
/// frame into); `tile_size_bytes_minus_1` MUST be the same value passed to
/// [`build_tile_group_multi`] for this frame's tile bitstreams — the two
/// are independently-computed byte-identical siblings, and disagreement
/// between them produces a non-decodable stream. Both are unused (any
/// value is fine, `0` by convention) when `tile_rows_log2 == 0`.
/// The chroma delta-q a frame header carries, TIED to the sequence header's
/// `separate_uv_delta_q` bit — the two must agree or a conforming decoder
/// desyncs, so the type makes disagreeing impossible.
///
/// Spec 5.9.12: `diff_uv_delta` is read ONLY when the SH signalled
/// `separate_uv_delta_q = 1`. Writing the `diff_uv_delta` bit under a SH that
/// signalled 0 shifts every following bit — which is exactly the bug this
/// enum replaced: the mainline tune-IQ chroma-q derivation started producing
/// non-zero deltas, they were emitted through the fork's four-delta form, and
/// all 60 cells of `tools/variance_boost_recon.sh` went to DECODE FAILED
/// (CI run 33220828356).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaQSignal {
    /// SH `separate_uv_delta_q = 0` (MAINLINE): ONE `(dc, ac)` pair, read as
    /// `DeltaQUDc`/`DeltaQUAc` and reused for V because `diff_uv_delta` is 0
    /// without being coded. No `diff_uv_delta` bit is written.
    ///
    /// This is what mainline C produces: `rc_crf_cqp.c:600-601` assigns the
    /// SAME value to `delta_q_{dc,ac}[1]` and `[2]`.
    Shared { dc: i8, ac: i8 },
    /// SH `separate_uv_delta_q = 1` (the svt-av1-hdr fork): `diff_uv_delta = 1`
    /// then independent `[u_dc, u_ac, v_dc, v_ac]`. The fork's U delta carries
    /// a further `+12`, so U and V genuinely differ.
    Separate([i8; 4]),
}

impl ChromaQSignal {
    /// True when every delta is zero — the frame is still CodedLossless-eligible.
    pub fn is_zero(&self) -> bool {
        match self {
            Self::Shared { dc, ac } => *dc == 0 && *ac == 0,
            Self::Separate(d) => *d == [0; 4],
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn write_frame_header_full_lr_sb(
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
    chroma_q: Option<ChromaQSignal>,
    delta_q_res: Option<u8>,
    // `frm_hdr->delta_lf_params.delta_lf_present` — signaled only when
    // `delta_q_res.is_some()` (spec 5.9.18 nests it inside delta_q_params).
    // C never sets it (resource_coordination_process.c:434-441); the port
    // keeps C's syntax and every caller passes false.
    delta_lf: bool,
    qm: Option<[u8; 3]>,
    fgs: Option<&FilmGrainParams>,
    tile_rows_log2: u8,
    tile_cols_log2: u8,
    tile_size_bytes_minus_1: u8,
    // Superblock size in PIXELS (64 or 128); must match the SH's
    // `use_128x128_superblock`.
    sb_size: u32,
    // `frm_hdr->tx_mode == TX_MODE_SELECT` — see `frame_header_bits_lr`.
    tx_mode_select: bool,
    // `None` -> KEY_FRAME (the historical behaviour of every caller);
    // `Some(..)` -> INTER_FRAME. See [`InterSignal`].
    inter: Option<&InterSignal>,
) -> Vec<u8> {
    let mut wb = frame_header_bits_lr(
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
        chroma_q,
        delta_q_res,
        delta_lf,
        qm,
        fgs,
        tile_rows_log2,
        tile_cols_log2,
        tile_size_bytes_minus_1,
        sb_size,
        tx_mode_select,
        inter,
    );
    // This header is embedded in an OBU_FRAME: the spec requires
    // byte_alignment() (zero bits only) between frame_header and tile_group —
    // trailing_bits (with its leading 1) is only for standalone
    // OBU_FRAME_HEADER. C: write_frame_header_obu(appendTrailingBits=0).
    byte_align_zero(&mut wb);
    wb.into_data()
}

/// Key-frame form of [`write_frame_header_full_lr_sb`] — `inter = None`.
///
/// Every pre-inter caller goes through here, so the key-frame layout is
/// byte-identical by construction.
#[allow(clippy::too_many_arguments)]
pub fn write_key_frame_header_full_lr_sb(
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
    chroma_q: Option<ChromaQSignal>,
    delta_q_res: Option<u8>,
    qm: Option<[u8; 3]>,
    fgs: Option<&FilmGrainParams>,
    tile_rows_log2: u8,
    tile_cols_log2: u8,
    tile_size_bytes_minus_1: u8,
    sb_size: u32,
    tx_mode_select: bool,
) -> Vec<u8> {
    write_frame_header_full_lr_sb(
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
        chroma_q,
        delta_q_res,
        false, // delta_lf: compat signature predates the extension
        qm,
        fgs,
        tile_rows_log2,
        tile_cols_log2,
        tile_size_bytes_minus_1,
        sb_size,
        tx_mode_select,
        None,
    )
}

/// 64px-superblock form of [`write_key_frame_header_full_lr_sb`] — the
/// historical signature, kept so every existing caller and test is
/// byte-identical by construction.
#[allow(clippy::too_many_arguments)]
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
    chroma_q: Option<ChromaQSignal>,
    delta_q_res: Option<u8>,
    qm: Option<[u8; 3]>,
    fgs: Option<&FilmGrainParams>,
    tile_rows_log2: u8,
    tile_cols_log2: u8,
    tile_size_bytes_minus_1: u8,
) -> Vec<u8> {
    write_key_frame_header_full_lr_sb(
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
        chroma_q,
        delta_q_res,
        qm,
        fgs,
        tile_rows_log2,
        tile_cols_log2,
        tile_size_bytes_minus_1,
        64,
        // The allintra arm's unconditional TX_MODE_SELECT; this wrapper is the
        // pre-video signature and every caller of it is on that arm.
        true,
    )
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
    frame_header_bits_lr(
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
        None,  // chroma_q: mainline zero-delta bit pattern
        None,  // delta_q_res: no per-SB delta-q
        false, // delta_lf: no delta-q -> delta_lf unsignaled
        None,  // qm: quant matrices off
        None,  // fgs: no film grain
        0,     // tile_rows_log2: test wrapper stays single-tile
        0,     // tile_cols_log2: ditto
        0,     // tile_size_bytes_minus_1: unused when NumTiles == 1
        64,
        true, // tx_mode_select: the allintra arm's unconditional TX_MODE_SELECT
        None, // inter: this test wrapper is key-frame only
    )
}

/// Everything `uncompressed_header()` writes for a NON-key frame that a key
/// frame does not (AV1 spec 5.9.2; C `write_uncompressed_header_obu`,
/// `Codec/entropy_coding.c:3299`).
///
/// Passing `Some(..)` to [`frame_header_bits_lr`] selects `frame_type =
/// INTER_FRAME` and every field below; `None` keeps the key-frame layout
/// byte-for-byte, so no existing caller moves.
///
/// **Presence, not just value.** The three `Option<bool>` fields are the
/// spec's conditionally-read bits: `None` means the decoder reads NO bit there
/// and infers 0. Spelling them as plain `bool` would have made a wrong
/// presence decision invisible — and in a frame header one missing bit shifts
/// every field after it, which is the failure mode `pipeline.rs`'s inter
/// refusal was written against.
#[derive(Debug, Clone)]
pub struct InterSignal {
    /// C update_parameters=0: reuse this DPB slot with the new random seed.
    pub film_grain_ref_idx: Option<u8>,
    /// FH `show_frame`. `false` on a random-access picture coded ahead of
    /// its display position (a hidden pyramid layer); the decoder then reads
    /// `showable_frame` — always 1 here, since every hidden picture is shown
    /// later through `show_existing_frame`. `true` on every low-delay frame.
    pub show_frame: bool,
    /// FH `error_resilient_mode`. C derives it; the low-delay CQP GOP this
    /// port encodes writes 0, which is what makes `primary_ref_frame` present.
    pub error_resilient_mode: bool,
    /// FH `order_hint`, `OrderHintBits` wide.
    pub order_hint: u8,
    /// FH `primary_ref_frame` (3 bits) — the reference whose END-OF-FRAME CDFs
    /// this frame inherits. Written only when `!error_resilient_mode`.
    /// `PRIMARY_REF_NONE` (7) means "start from the default CDFs".
    pub primary_ref_frame: u8,
    /// FH `refresh_frame_flags` (8 bits) — C `pic.rps.refresh_frame_mask`.
    pub refresh_frame_flags: u8,
    /// FH `ref_frame_idx[0..7]`, 3 bits each — C `pic.rps.ref_dpb_index[]`,
    /// in LAST, LAST2, LAST3, GOLDEN, BWDREF, ALTREF2, ALTREF order.
    pub ref_frame_idx: [u8; 7],
    /// FH `allow_high_precision_mv`.
    pub allow_high_precision_mv: bool,
    /// `read_interpolation_filter()`: `None` = `is_filter_switchable = 1` (no
    /// further bits); `Some(f)` = the 2-bit `interpolation_filter`.
    pub interpolation_filter: Option<u8>,
    /// FH `is_motion_mode_switchable`.
    pub is_motion_mode_switchable: bool,
    /// FH `use_ref_frame_mvs` — `None` when `error_resilient_mode ||
    /// !enable_ref_frame_mvs`, where the decoder reads no bit.
    pub use_ref_frame_mvs: Option<bool>,
    /// FH `reference_select` (`frame_reference_mode()`).
    pub reference_select: bool,
    /// FH `skip_mode_present` — `None` when `skipModeAllowed` is 0, where the
    /// decoder reads no bit (spec `skip_mode_params()`).
    pub skip_mode_present: Option<bool>,
    /// FH `allow_warped_motion` — `None` when `error_resilient_mode ||
    /// !enable_warped_motion`, where the decoder reads no bit.
    pub allow_warped_motion: Option<bool>,
    /// `global_motion_params()`: this frame's warp model per reference,
    /// indexed by `MvReferenceFrame` (entry 0, INTRA_FRAME, is unused).
    ///
    /// C `pcs->ppcs->global_motion[]`, built by
    /// `port_global_me::set_global_motion_field` from the search's own verdict.
    /// This used to be seven `is_global` BITS with a `debug_assert` that they
    /// were all false, and the pipeline refused any frame where C's search had
    /// found a model — see `gm_search_config_error`.
    pub global_motion: [crate::port_entropy_inter::gm::WarpParams; 8],
    /// C `pcs->child_pcs->ref_global_motion[]` — the PRIMARY-REF picture's own
    /// saved models, which each parameter is delta-coded against. IDENTITY
    /// throughout when `primary_ref_frame == PRIMARY_REF_NONE` or when that
    /// picture was an I_SLICE (`pic_manager_process.c:831`).
    pub ref_global_motion: [crate::port_entropy_inter::gm::WarpParams; 8],
    /// `is_global[ref]` for LAST..ALTREF, i.e. `global_motion[ref + 1].wmtype
    /// != IDENTITY`.
    ///
    /// READ-ONLY MIRROR, kept because it is public API: this used to be the
    /// whole of the port's `global_motion_params()` and the writer no longer
    /// reads it. Setting it changes no bit — [`Self::global_motion`] is what
    /// gets coded, and `is_global` is the first bit of each model's own
    /// encoding. [`Self::sync_is_global`] refreshes it after the models are
    /// filled.
    pub is_global: [bool; 7],
}

impl Default for InterSignal {
    /// Hand-implemented (not derived) so [`Self::show_frame`] defaults to
    /// `true`: a `..Default::default()` construction site must describe a
    /// SHOWN frame, which is every frame the pre-RA port emitted.
    fn default() -> Self {
        Self {
            film_grain_ref_idx: None,
            show_frame: true,
            error_resilient_mode: false,
            order_hint: 0,
            primary_ref_frame: 0,
            refresh_frame_flags: 0,
            ref_frame_idx: [0; 7],
            allow_high_precision_mv: false,
            interpolation_filter: None,
            is_motion_mode_switchable: false,
            use_ref_frame_mvs: None,
            reference_select: false,
            skip_mode_present: None,
            allow_warped_motion: None,
            global_motion: Default::default(),
            ref_global_motion: Default::default(),
            is_global: [false; 7],
        }
    }
}

impl InterSignal {
    /// Refresh the [`Self::is_global`] mirror from [`Self::global_motion`].
    ///
    /// Called once the pipeline has filled the models. The mirror exists only
    /// so the field this struct used to carry keeps answering truthfully; no
    /// coded bit depends on it.
    pub fn sync_is_global(&mut self) {
        for (i, flag) in self.is_global.iter_mut().enumerate() {
            *flag = self.global_motion[i + 1].wmtype
                != crate::port_entropy_inter::modes::TransformationType::Identity;
        }
    }
}

/// Body of [`write_key_frame_header_full_lr`] (see the compat wrapper above).
///
/// `inter` is `None` for a key frame and `Some(..)` for an INTER frame; see
/// [`InterSignal`].
#[allow(clippy::too_many_arguments)]
fn frame_header_bits_lr(
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
    chroma_q: Option<ChromaQSignal>,
    delta_q_res: Option<u8>,
    // `frm_hdr->delta_lf_params.delta_lf_present` — only meaningful with
    // `delta_q_res: Some` (the syntax nests inside delta_q_params).
    delta_lf: bool,
    qm: Option<[u8; 3]>,
    fgs: Option<&FilmGrainParams>,
    tile_rows_log2: u8,
    tile_cols_log2: u8,
    tile_size_bytes_minus_1: u8,
    // Superblock size in PIXELS (64 or 128) — the tile limits are
    // SB-derived (spec 5.9.15), so this must match the SH's
    // `use_128x128_superblock`.
    sb_size: u32,
    // `frm_hdr->tx_mode == TX_MODE_SELECT` (`crate::txs_arm::tx_mode_select`).
    // ALWAYS true on the allintra arm, which is why this used to be a literal.
    tx_mode_select: bool,
    // `None` -> KEY_FRAME, the historical layout. `Some(..)` -> INTER_FRAME.
    inter: Option<&InterSignal>,
) -> BitWriter {
    let mut wb = BitWriter::new();

    if !reduced_sh {
        // ---- Full frame header preamble ----
        wb.write_bit(false); // show_existing_frame = 0
        // frame_type: KEY_FRAME (0) or INTER_FRAME (1).
        wb.write_bits(u32::from(inter.is_some()), 2);
        wb.write_bit(inter.map_or(true, |it| it.show_frame)); // show_frame
        if let Some(it) = inter {
            // showable_frame: read iff show_frame = 0 (C
            // entropy_coding.c:3344-3347). Every hidden picture this port
            // codes is shown later through show_existing_frame, so 1.
            if !it.show_frame {
                wb.write_bit(true);
            }
            // error_resilient_mode: implicit 1 for KEY_FRAME with
            // show_frame = 1 (and for SWITCH_FRAME); READ for every other
            // frame type. C's low-delay CQP inter frames carry 0, which is
            // what makes `primary_ref_frame` present a few fields down.
            wb.write_bit(it.error_resilient_mode);
        }
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
        wb.write_bits(
            u32::from(inter.map_or(0, |it| it.order_hint)),
            ORDER_HINT_BITS,
        ); // order_hint (0 for a key frame)
        if let Some(it) = inter {
            // primary_ref_frame (spec 5.9.2): read when
            // `!FrameIsIntra && !error_resilient_mode`. It names the reference
            // whose END-OF-FRAME CDFs this frame starts from, so getting it
            // wrong desynchronises the entropy decoder at the first symbol —
            // it is a hard bitstream field, not a hint.
            if !it.error_resilient_mode {
                wb.write_bits(u32::from(it.primary_ref_frame), 3);
            }
            // refresh_frame_flags: implicit `allFrames` ONLY for SWITCH_FRAME
            // and a SHOWN key frame; written for every inter frame.
            wb.write_bits(u32::from(it.refresh_frame_flags), 8);
            // The `ref_order_hint[NUM_REF_FRAMES]` map that follows is read
            // only when `error_resilient_mode && enable_order_hint`. This port
            // does not produce error-resilient inter frames, and emitting the
            // map wrong would shift every following field, so the branch is an
            // assertion rather than a guess.
            debug_assert!(
                !it.error_resilient_mode,
                "error-resilient inter frames need the ref_order_hint[8] map \
                 (spec 5.9.2); this writer does not emit it"
            );
        }
        // primary_ref_frame: NOT signaled for KEY_FRAME with error_resilient=1
        //   (implicit PRIMARY_REF_NONE)
        //
        // refresh_frame_flags: NOT signaled either. Spec 5.9.2 and C
        // (entropy_coding.c:3404-3407, `if (!show_frame)`) leave it IMPLICIT
        // at allFrames for a SWITCH frame or a SHOWN key frame — and this
        // writer only emits shown key frames. Writing the 8 bits put an
        // illegal field in the stream and shifted every following one, which
        // is the third of the four defects the inter refusal in pipeline.rs
        // names. MEASURED: it was the first divergence in the video-mode
        // frame OBU, at bit 14, immediately after order_hint.
    }

    if let Some(it) = inter {
        // ---- the INTER arm of spec 5.9.2 ----
        // Order is load-bearing and differs from the intra arm: the reference
        // indices come BEFORE frame_size()/render_size(), not after.
        //
        // frame_refs_short_signaling: read when the SH has enable_order_hint
        // (it does — the key frame this port already emits byte-identically
        // signals it). C never sets it (`entropy_coding.c:3492-3497` writes
        // `pcs->frame_refs_short_signaling`, left 0), so all seven indices are
        // written explicitly.
        wb.write_bit(false); // frame_refs_short_signaling = 0
        for idx in it.ref_frame_idx {
            debug_assert!(idx < 8, "ref_frame_idx is a DPB slot, 0..=7");
            wb.write_bits(u32::from(idx), 3);
        }
        // frame_id_numbers_present_flag = 0 in the SH -> no delta_frame_id.
        // frame_size_override_flag = 0 -> frame_size(); render_size().
        sc.superres.write(&mut wb);
        wb.write_bit(false); // render_and_frame_size_different = 0
        // force_integer_mv is 0 here: it is only written when
        // allow_screen_content_tools is set, and the writer above emits a
        // literal 0 for it in that case.
        wb.write_bit(it.allow_high_precision_mv);
        // read_interpolation_filter()
        match it.interpolation_filter {
            None => wb.write_bit(true), // is_filter_switchable = 1
            Some(f) => {
                debug_assert!(f < 4, "interpolation_filter is 2 bits");
                wb.write_bit(false);
                wb.write_bits(u32::from(f), 2);
            }
        }
        wb.write_bit(it.is_motion_mode_switchable);
        // use_ref_frame_mvs: no bit when error_resilient_mode ||
        // !enable_ref_frame_mvs.
        if let Some(v) = it.use_ref_frame_mvs {
            wb.write_bit(v);
        }
        // allow_intrabc is an intra-frame field only.
    } else {
        // ---- frame_size() ----
        // frame_size_override_flag = 0 → use SH dimensions, no bits.
        // superres_params() (spec 5.9.8): one `use_superres` bit when the SH
        // has the tool on, plus 3 bits of `coded_denom` when it is set.
        // Nothing is written when `enable_superres` is off (every current gate
        // cell), so this is bit-for-bit the previous behaviour there.
        sc.superres.write(&mut wb);

        // ---- render_size() ----
        wb.write_bit(false); // render_and_frame_size_different = 0

        // allow_intrabc: signaled iff allow_screen_content_tools AND the frame
        // is NOT superres-scaled (spec 5.9.11 `UpscaledWidth == FrameWidth`; C
        // entropy_coding.c:3464-3466, after write_frame_size). IntraBC cannot
        // be combined with superres because the block copy would read the
        // pre-upscale grid.
        if sc.allow_screen_content_tools && sc.superres.denom.is_none() {
            wb.write_bit(sc.allow_intrabc);
        }
    }

    // ---- disable_frame_end_update_cdf (spec 5.9.2) ----
    // C's `might_bwd_adapt` (entropy_coding.c:3553-3559):
    //     !reduced_still_picture_header && !disable_cdf_update
    // and only then is the bit written, as
    // `refresh_frame_context == REFRESH_FRAME_CONTEXT_DISABLED`. The picture
    // manager assigns REFRESH_FRAME_CONTEXT_BACKWARD to coded pictures
    // (pic_manager_process.c:868,875), so the bit is 0 on this path.
    //
    // In the reduced (still) header the field is IMPLICIT 1 and no bit is
    // written, which is why every still cell was already byte-identical
    // without it. Omitting it from the video header shifted tile_info() and
    // every following field by one bit — the fourth of the four defects the
    // inter refusal in pipeline.rs names.
    //
    // `disable_cdf_update` is written as 0 above, so the guard reduces to
    // `!reduced_sh` today; it is spelled out anyway so that making
    // disable_cdf_update dynamic cannot silently desync the header.
    let disable_cdf_update = false;
    if !reduced_sh && !disable_cdf_update {
        wb.write_bit(false); // refresh_frame_context != DISABLED
    }

    // ---- tile_info() ----
    write_tile_info(
        &mut wb,
        width,
        height,
        tile_rows_log2,
        tile_cols_log2,
        tile_size_bytes_minus_1,
        sb_size,
    );

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
        // With SH separate_uv_delta_q=1 the decoder first reads
        // diff_uv_delta, then U and (if diff) V deltas. delta_q syntax
        // (spec 5.9.13 read_delta_q): 1-bit delta_coded, then su(1+6)
        // = 7-bit two's-complement inv_signed_literal.
        let write_delta = |wb: &mut BitWriter, d: i8| {
            if d == 0 {
                wb.write_bit(false); // delta_coded = 0
            } else {
                wb.write_bit(true); // delta_coded = 1
                wb.write_bits((d as i32 & 0x7f) as u32, 7); // su(1+6)
            }
        };
        match chroma_q {
            None => {
                wb.write_bit(false); // DeltaQUDc: delta_coded = 0
                wb.write_bit(false); // DeltaQUAc: delta_coded = 0
            }
            // SH separate_uv_delta_q = 0: NO diff_uv_delta bit (the decoder
            // does not read one), U deltas only, V reuses them.
            Some(ChromaQSignal::Shared { dc, ac }) => {
                write_delta(&mut wb, dc);
                write_delta(&mut wb, ac);
            }
            // SH separate_uv_delta_q = 1 (fork): diff_uv_delta then all four.
            Some(ChromaQSignal::Separate([u_dc, u_ac, v_dc, v_ac])) => {
                wb.write_bit(true); // diff_uv_delta
                for d in [u_dc, u_ac, v_dc, v_ac] {
                    write_delta(&mut wb, d);
                }
            }
        }
    }
    // NumPlanes=1 (mono_chrome): no DeltaQUDc, DeltaQUAc
    // [SVT_HDR_MODE] quantizer matrices (spec 5.9.12; C entropy_coding.c:
    // 2451): qm_y + qm_u always, qm_v only when the SH signaled
    // separate_uv_delta_q=1 (the fork chroma-q path — chroma_q Some).
    match qm {
        None => wb.write_bit(false), // using_qmatrix = 0
        Some([qm_y, qm_u, qm_v]) => {
            wb.write_bit(true); // using_qmatrix = 1
            wb.write_bits(u32::from(qm_y), 4);
            wb.write_bits(u32::from(qm_u), 4);
            // qm_v is written ONLY when the SH signalled
            // separate_uv_delta_q = 1 — the same bit that gates
            // diff_uv_delta, so it follows the SEPARATE variant, not merely
            // "chroma deltas exist". Keying it on `is_some()` would emit a
            // stray 4-bit field on the mainline Shared path.
            if matches!(chroma_q, Some(ChromaQSignal::Separate(_))) {
                wb.write_bits(u32::from(qm_v), 4);
            } else {
                debug_assert_eq!(qm_u, qm_v, "qm_v needs separate_uv_delta_q");
            }
        }
    }

    // ---- segmentation_params() ----
    wb.write_bit(false); // segmentation_enabled = 0

    // ---- delta_q_params() ----
    // Spec: delta_q_present is only signaled when base_q_idx > 0.
    // [SVT_HDR_MODE] fork variance boost signals per-SB delta-q:
    // delta_q_present=1 + 2-bit log2(delta_q_res) (spec 5.9.17).
    if base_qindex > 0 {
        match delta_q_res {
            None => wb.write_bit(false), // delta_q_present = 0
            Some(res) => {
                debug_assert!(matches!(res, 1 | 2 | 4 | 8));
                wb.write_bit(true); // delta_q_present = 1
                wb.write_bits(res.trailing_zeros(), 2); // delta_q_res log2
                // delta_lf_params(): read only when delta_q_present &&
                // !allow_intrabc (spec 5.9.18). aom `--delta-lf-mode`
                // signals present + delta_lf_res log2 (2 -> 1) +
                // delta_lf_multi (0); C writes the same layout with
                // present hardwired 0 (entropy_coding.c:3578-3588).
                if !sc.allow_intrabc {
                    wb.write_bit(delta_lf); // delta_lf_present
                    if delta_lf {
                        wb.write_bits(1, 2); // delta_lf_res = 2 -> log2 = 1
                        wb.write_bit(false); // delta_lf_multi = 0
                    }
                }
            }
        }
    }
    // delta_lf_params(): not signaled when delta_q_present=0

    // ---- CodedLossless / AllLossless (spec 5.9.2 tail of
    // read_quantization_params / segmentation: `LosslessArray[seg] =
    // qindex == 0 && DeltaQYDc == 0 && DeltaQUAc == 0 && DeltaQUDc == 0 &&
    // DeltaQVAc == 0 && DeltaQVDc == 0`, CodedLossless = all segments;
    // AllLossless = CodedLossless && FrameWidth == UpscaledWidth). Segmentation
    // is off (above), so every segment carries base_q_idx; DeltaQYDc is
    // always 0 here and the chroma deltas are `chroma_q`. C:
    // md_config_process.c:992-1017 (`coded_lossless = lossless[0] =
    // !base_q_idx` when segmentation is off) and entropy_coding.c:3594-3612.
    // Issue #5 chunk 1: the header half of the coded-lossless envelope —
    // pipeline.rs still refuses base_qindex 0 until the tile half (TX_4X4
    // WHT, no tx_size / tx_type symbols) is ported and byte-verified.
    let coded_lossless = base_qindex == 0 && chroma_q.is_none_or(|d| d.is_zero());
    let all_lossless = coded_lossless && sc.superres.denom.is_none();

    // ---- loop_filter_params() ----
    // When CodedLossless or allow_intrabc: NO loop filter bits (spec 5.9.11
    // sets the levels to 0; C entropy_coding.c:3597 skips encode_loopfilter).
    // Field set matches C encode_loopfilter (entropy_coding.c:2338) and
    // spec 5.9.11; libaom setup_loopfilter (decodeframe.c:1766) reads it.
    if !sc.allow_intrabc && !coded_lossless {
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
    // signals enable_cdef=1 and this header is neither CodedLossless nor
    // allow_intrabc — either one means NO cdef bits (spec 5.9.19 early-out;
    // C entropy_coding.c:3598-3600 under `!coded_lossless`).
    if !sc.allow_intrabc && !coded_lossless {
        // `damping == 0` is C's "the pick never ran" state, NOT an
        // out-of-range value — see `cdef::CdefFrameParams::never_picked`.
        debug_assert!(
            (3..=6).contains(&cdef.damping) || cdef.damping == 0,
            "cdef_damping out of range"
        );
        debug_assert_eq!(cdef.strengths.len(), 1usize << cdef.bits);
        // C `svt_aom_wb_write_literal(wb, cdef_damping - 3, 2)`
        // (entropy_coding.c:2349) on a `uint8_t` promoted to `int`: at
        // damping 0 that is `-3`, whose low two bits are 1. The wrapping
        // subtraction reproduces it instead of underflowing in debug and
        // silently differing in release.
        wb.write_bits(u32::from(cdef.damping.wrapping_sub(3)) & 3, 2); // cdef_damping_minus_3
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
    // AllLossless also folds in (spec 5.9.20: `if AllLossless || allow_intrabc
    // || !enable_restoration` -> no lr bits; C entropy_coding.c:3594 codes
    // restoration only in the `!all_lossless` arm).
    if lr.enabled && !sc.allow_intrabc && !all_lossless {
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
            // C encode_restoration_mode (entropy_coding.c:2225-2236):
            //
            //     if (sb_size == 64) wb_write_bit(unit_size > 64);
            //     if (unit_size > 64) wb_write_bit(unit_size > 128);
            //
            // The FIRST bit is written ONLY at SB64 (task #91). At SB128
            // the spec's RESTORATION_UNITSIZE_MAX-relative encoding starts
            // one step up, because a restoration unit may never be smaller
            // than the superblock (C asserts `unit_size >= sb_size`), so
            // `unit_size > 64` is not a free choice — it is implied. Writing
            // it anyway shifts every following header bit by one and
            // silently corrupts tx_mode_select / reduced_tx_set (MEASURED:
            // exactly the `diag 512x384 q55 p0` and `gradient 512x384 q55
            // p0` divergences, whose tile payloads were already
            // byte-identical).
            debug_assert!(u32::from(lr.unit_size) >= sb_size);
            if sb_size == 64 {
                wb.write_bit(lr.unit_size > 64);
            }
            if lr.unit_size > 64 {
                wb.write_bit(lr.unit_size > 128);
            }
        }
        if !chroma_none {
            wb.write_bit(lr.uv_size_differs);
        }
    }

    // ---- read_tx_mode() ----
    // CodedLossless -> TxMode = ONLY_4X4 and NO bit (spec 5.9.21; C
    // entropy_coding.c:3608-3612). Otherwise TX_MODE_SELECT, like C: SVT
    // always sets frm_hdr->tx_mode = TX_MODE_SELECT at these presets ("Use
    // TX_MODE_SELECT even when txs_level == 0", enc_mode_config.c:15140-15143;
    // written at entropy_coding.c:3659). The tile walk then codes a per-block
    // tx_depth symbol for every bsize > 4x4 (always depth 0 = largest —
    // matching what the LARGEST mode implied, but now in C's syntax).
    if !coded_lossless {
        // The ALLINTRA arm sets TX_MODE_SELECT unconditionally ("Use
        // TX_MODE_SELECT even when txs_level == 0, as the decision may change
        // from OFF to Fastest at the SB level", enc_mode_config.c:10025). The
        // VIDEO arm sets it only when `pcs->txs_level != 0` (`:9194`), i.e.
        // TX_MODE_LARGEST at preset 10 and up — where this writer used to emit
        // a literal 1 and then code per-block tx_depth symbols that
        // TX_MODE_LARGEST forbids.
        wb.write_bit(tx_mode_select);
    }

    // ---- frame_reference_mode() + skip_mode_params() + allow_warped_motion ----
    // Intra frames write none of these (spec 5.9.2 gates each on
    // `!FrameIsIntra`); C: entropy_coding.c:3615-3629.
    if let Some(it) = inter {
        wb.write_bit(it.reference_select);
        // skip_mode_present: no bit when `skipModeAllowed` is 0.
        if let Some(v) = it.skip_mode_present {
            wb.write_bit(v);
        }
        // allow_warped_motion: no bit when error_resilient_mode ||
        // !enable_warped_motion.
        if let Some(v) = it.allow_warped_motion {
            wb.write_bit(v);
        }
    }

    wb.write_bit(false); // reduced_tx_set = 0

    // ---- global_motion_params() (spec 5.9.24) ----
    // Intra frames write nothing. For an inter frame, one `is_global` bit per
    // reference, and a set bit is followed by the type and up to six
    // parameters, each delta-coded against the primary reference picture's own
    // model. This used to write seven zero bits under a `debug_assert` that
    // the frame had no model, with the pipeline refusing any frame where C's
    // search found one; `port_entropy_inter::gm::write_global_motion` is that
    // coding, and `tools/global_motion_gate.sh` pins the result against dav1d.
    if let Some(it) = inter {
        crate::port_entropy_inter::gm::write_global_motion(
            &mut wb,
            &it.global_motion,
            &it.ref_global_motion,
            // C `write_global_motion` reads `pcs->ppcs->frm_hdr.primary_ref_frame`
            // to choose between the reference models and IDENTITY.
            it.primary_ref_frame,
            it.allow_high_precision_mv,
        );
    }

    // ---- film_grain_params() (spec 5.9.30) ---- read only when the SH
    // signaled film_grain_params_present && show_frame (C
    // entropy_coding.c:3698). The SH bit and this Option MUST agree.
    if let Some(fg) = fgs {
        write_film_grain_params(&mut wb, fg, inter);
    }

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

/// Two-argument tile_log2 (C `tile_log2(blk_size, target)`,
/// entropy_coding.c): smallest `k` such that `blk_size << k >= target`.
/// The single-arg [`tile_log2`] above is this with `blk_size = 1`.
fn tile_log2_blk(blk_size: u32, target: u32) -> u32 {
    let mut k = 0u32;
    while (u64::from(blk_size) << k) < u64::from(target) {
        k += 1;
    }
    k
}

/// Tile legality shared by validation and layout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TileLimits {
    /// Smallest legal `tile_cols_log2` (forced up by `MAX_TILE_WIDTH`).
    pub min_log2_tile_cols: u32,
    /// Largest legal `tile_cols_log2` (capped by `MAX_TILE_COLS`).
    pub max_log2_tile_cols: u32,
    /// Largest legal `tile_rows_log2` (capped by `MAX_TILE_ROWS`).
    pub max_log2_tile_rows: u32,
    /// Smallest legal total tile count, log2 (forced up by `MAX_TILE_AREA`).
    pub min_log2_tiles: u32,
}

impl TileLimits {
    /// C `svt_av1_get_tile_limits`. `sb_size` is in PIXELS (64 or 128).
    pub(crate) fn for_frame(width: u32, height: u32, sb_size: u32) -> Self {
        let sb_size_log2 = sb_size.trailing_zeros();
        let sb_cols = width.div_ceil(sb_size);
        let sb_rows = height.div_ceil(sb_size);
        let max_tile_width_sb = (MAX_TILE_WIDTH as u32) >> sb_size_log2;
        let max_tile_area_sb = (MAX_TILE_AREA as u32) >> (2 * sb_size_log2);
        let min_log2_tile_cols = tile_log2_blk(max_tile_width_sb, sb_cols);
        Self {
            min_log2_tile_cols,
            max_log2_tile_cols: tile_log2(sb_cols.min(MAX_TILE_COLS as u32)),
            max_log2_tile_rows: tile_log2(sb_rows.min(MAX_TILE_ROWS as u32)),
            min_log2_tiles: tile_log2_blk(max_tile_area_sb, sb_rows.saturating_mul(sb_cols))
                .max(min_log2_tile_cols),
        }
    }

    /// `Some(reason)` when no legal tile grid exists for this frame — i.e. when
    /// either axis has `min > max` and [`TileGrid::resolve`]'s clamp would
    /// panic. `requested_cols_log2` participates because the ROW minimum is
    /// derived from the CHOSEN column count, exactly as in `resolve`.
    pub(crate) fn untileable_reason(&self, requested_cols_log2: u8) -> Option<&'static str> {
        if self.min_log2_tile_cols > self.max_log2_tile_cols {
            return Some(
                "frame is too wide to tile: AV1 caps a tile at MAX_TILE_WIDTH (4096 px) and a \
                 frame at MAX_TILE_COLS (64) tile columns, so the widest encodable frame is \
                 64 * 4096 = 262144 px",
            );
        }
        let tile_cols_log2 =
            u32::from(requested_cols_log2).clamp(self.min_log2_tile_cols, self.max_log2_tile_cols);
        let min_log2_tile_rows = self.min_log2_tiles.saturating_sub(tile_cols_log2);
        if min_log2_tile_rows > self.max_log2_tile_rows {
            return Some(
                "frame is too large to tile: MAX_TILE_AREA forces more tiles than \
                 MAX_TILE_ROWS (64) tile rows can supply at this width",
            );
        }
        None
    }
}

/// The resolved uniform tile grid — a faithful port of C's
/// `svt_av1_get_tile_limits` + `svt_av1_calculate_tile_cols` +
/// `svt_av1_calculate_tile_rows`, driven in the order
/// `svt_aom_set_tile_info` drives them (entropy_coding.c:2450-2579).
///
/// The distinction this type exists to carry is **requested log2 vs
/// ACTUAL tile count**. C's tiling algorithm (the comment block at
/// `svt_aom_set_tile_info`) rounds the SB count up to `1 << log2`,
/// divides to get a uniform tile size, then *fills tiles of that size
/// until the picture ends* — so "the last tile could have smaller size,
/// and the final number of tiles could be less than tile_count". A 6-SB
/// dimension with `log2 = 2` yields `size_sb = 2` and therefore **3**
/// tiles, not 4.
///
/// Getting that wrong is not a byte-difference, it is a CORRUPT stream:
/// `context_update_tile_id` is written as `NumTiles - 1` in
/// `TileColsLog2 + TileRowsLog2` bits, so emitting `(1 << log2) - 1`
/// when fewer tiles exist makes the id exceed `NumTiles - 1` and every
/// conforming decoder rejects the frame ("Invalid context_update_tile").
/// The pre-task-#96 rows-only path did exactly that for any frame whose
/// SB-row count was not a multiple of the requested tile count.
///
/// All spans are in SUPERBLOCK units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileGrid {
    /// Resolved `TileColsLog2` (clamped into the legal range).
    pub tile_cols_log2: u8,
    /// Resolved `TileRowsLog2` (clamped into the legal range).
    pub tile_rows_log2: u8,
    /// ACTUAL tile-column count — may be `< 1 << tile_cols_log2`.
    pub tile_cols: usize,
    /// ACTUAL tile-row count — may be `< 1 << tile_rows_log2`.
    pub tile_rows: usize,
    /// Uniform tile width in SBs (the last column may be narrower).
    pub tile_width_sb: usize,
    /// Uniform tile height in SBs (the last row may be shorter).
    pub tile_height_sb: usize,
    /// Frame width in SBs.
    pub sb_cols: usize,
    /// Frame height in SBs.
    pub sb_rows: usize,
    min_log2_tile_cols: u32,
    max_log2_tile_cols: u32,
    /// Recomputed as `max(min_log2_tiles - TileColsLog2, 0)` AFTER the
    /// columns are resolved — C does this both in
    /// `svt_av1_calculate_tile_cols` and again in
    /// `write_tile_info_max_tile` (entropy_coding.c:2417).
    min_log2_tile_rows: u32,
    max_log2_tile_rows: u32,
}

impl TileGrid {
    /// Resolve a requested `(TileRowsLog2, TileColsLog2)` against the
    /// frame geometry. `sb_size` is the superblock size in PIXELS (64 or
    /// 128); the tile limits are SB-derived, so `max_tile_width_sb`
    /// HALVES and `max_tile_area_sb` QUARTERS at SB128 (C shifts by the
    /// PIXEL-domain `sb_size_log2`).
    ///
    /// Out-of-range requests are CLAMPED, exactly like C, rather than
    /// rejected — so a nonsense request degrades to the largest grid the
    /// frame supports instead of producing a bitstream inconsistent with
    /// what was actually encoded.
    pub fn resolve(
        width: u32,
        height: u32,
        sb_size: u32,
        requested_rows_log2: u8,
        requested_cols_log2: u8,
    ) -> Self {
        debug_assert!(sb_size == 64 || sb_size == 128, "sb_size must be 64 or 128");
        // C: sb_size_log2 = log2_sb_size(MI units) + MI_SIZE_LOG2(2). For
        // a 64px SB log2_sb_size = log2(64/4) = 4 -> 6; for 128px it is
        // log2(128/4) = 5 -> 7 (the PIXEL log2).
        let limits = TileLimits::for_frame(width, height, sb_size);
        assert!(
            limits.untileable_reason(requested_cols_log2).is_none(),
            "TileGrid::resolve requires a tileable frame"
        );
        let TileLimits {
            min_log2_tile_cols,
            max_log2_tile_cols,
            max_log2_tile_rows,
            min_log2_tiles,
        } = limits;
        let sb_cols = width.div_ceil(sb_size);
        let sb_rows = height.div_ceil(sb_size);

        // --- columns first (svt_aom_set_tile_info:2555-2560) ---
        let tile_cols_log2 =
            u32::from(requested_cols_log2).clamp(min_log2_tile_cols, max_log2_tile_cols);
        // svt_av1_calculate_tile_cols: size_sb = ceil(sb_cols / 2^log2)
        // via ALIGN_POWER_OF_TWO, then fill until the picture ends.
        let tile_width_sb = sb_cols.div_ceil(1 << tile_cols_log2).max(1);
        let tile_cols = sb_cols.div_ceil(tile_width_sb) as usize;

        // --- then rows, with min recomputed against the chosen cols ---
        let min_log2_tile_rows = min_log2_tiles.saturating_sub(tile_cols_log2);
        let tile_rows_log2 =
            u32::from(requested_rows_log2).clamp(min_log2_tile_rows, max_log2_tile_rows);
        let tile_height_sb = sb_rows.div_ceil(1 << tile_rows_log2).max(1);
        let tile_rows = sb_rows.div_ceil(tile_height_sb) as usize;

        Self {
            tile_cols_log2: tile_cols_log2 as u8,
            tile_rows_log2: tile_rows_log2 as u8,
            tile_cols,
            tile_rows,
            tile_width_sb: tile_width_sb as usize,
            tile_height_sb: tile_height_sb as usize,
            sb_cols: sb_cols as usize,
            sb_rows: sb_rows as usize,
            min_log2_tile_cols,
            max_log2_tile_cols,
            min_log2_tile_rows,
            max_log2_tile_rows,
        }
    }

    /// ACTUAL total tile count (`tile_rows * tile_cols`) — the value
    /// `context_update_tile_id` and the tile group's size-prefix count
    /// both derive from.
    pub fn num_tiles(&self) -> usize {
        self.tile_rows * self.tile_cols
    }

    /// SB-column span `[start, end)` of tile column `tc`. The last
    /// column is clipped to the frame.
    pub fn col_span(&self, tc: usize) -> (usize, usize) {
        let start = tc * self.tile_width_sb;
        (start, ((tc + 1) * self.tile_width_sb).min(self.sb_cols))
    }

    /// SB-row span `[start, end)` of tile row `tr`. The last row is
    /// clipped to the frame.
    pub fn row_span(&self, tr: usize) -> (usize, usize) {
        let start = tr * self.tile_height_sb;
        (start, ((tr + 1) * self.tile_height_sb).min(self.sb_rows))
    }

    /// The tile bounds, in LUMA mi (4px) units, of the tile CONTAINING the
    /// superblock at `(sb_row, sb_col)` — what every intra-availability
    /// predicate is scoped to (C `TileInfo`).
    ///
    /// ONE owner for this derivation: it is the same
    /// `start * sb_size / 4` / `min(end * sb_size / 4, dim / 4)` the per-tile
    /// `EntropyCtx` construction in `pipeline.rs` used to spell out inline,
    /// and the bd10 level re-encode post-pass (which walks the merged frame in
    /// raster SB order, so it cannot inherit a tile's context) needs the same
    /// answer per SB. Issue #18: that post-pass previously used
    /// `TileMi::whole_frame`, which predicts across tile edges a conforming
    /// decoder cannot see.
    ///
    /// `w`/`h` are the ALIGNED luma dims, matching the `EntropyCtx` sites.
    pub fn tile_mi_for_sb(
        &self,
        sb_row: usize,
        sb_col: usize,
        sb_size: usize,
        w: usize,
        h: usize,
    ) -> crate::intra_edge::TileMi {
        let (r0, r1) = self.row_span(sb_row / self.tile_height_sb);
        let (c0, c1) = self.col_span(sb_col / self.tile_width_sb);
        crate::intra_edge::TileMi {
            mi_row_start: r0 * sb_size / 4,
            mi_row_end: (r1 * sb_size / 4).min(h / 4),
            mi_col_start: c0 * sb_size / 4,
            mi_col_end: (c1 * sb_size / 4).min(w / 4),
        }
    }
}

/// Rows-only convenience over [`TileGrid::resolve`] at SB64 — clamps a
/// requested `TileRowsLog2` into `[minLog2TileRows, maxLog2TileRows]`
/// exactly like C, with `TileColsLog2 = 0`.
pub fn resolve_tile_rows_log2(width: u32, height: u32, requested_log2: u8) -> u8 {
    resolve_tile_rows_log2_sb(width, height, requested_log2, 64)
}

/// [`resolve_tile_rows_log2`] with an explicit superblock size (64 or 128
/// PIXELS).
pub fn resolve_tile_rows_log2_sb(width: u32, height: u32, requested_log2: u8, sb_size: u32) -> u8 {
    TileGrid::resolve(width, height, sb_size, requested_log2, 0).tile_rows_log2
}

/// C `mem_put_varsize` (entropy_coding.c:32): pack `val` into `sz` bytes,
/// little-endian, `sz` in `1..=4`.
fn put_varsize(sz: u8, val: u32) -> Vec<u8> {
    match sz {
        1 => alloc::vec![val as u8],
        2 => alloc::vec![val as u8, (val >> 8) as u8],
        3 => alloc::vec![val as u8, (val >> 8) as u8, (val >> 16) as u8],
        4 => val.to_le_bytes().to_vec(),
        _ => unreachable!("tile_size_bytes must be 1..=4"),
    }
}

/// C tile-size-prefix byte width selection (`write_tile_info`,
/// entropy_coding.c:2598-2610): the number of bytes needed to hold the
/// largest tile's RAW byte length (NOT `len - 1`) among every tile
/// EXCEPT the last (the last tile is never size-prefixed — spec 5.11.1).
/// Returns `tile_size_bytes_minus_1` (0..=3); the trailer literal AND
/// the actual prefix width (`+ 1`) both derive from this one value, and
/// BOTH the frame-header [`write_tile_info`] trailer and
/// [`build_tile_group_multi`] must agree on it — callers compute it once
/// (from the concrete per-tile byte lengths) and thread it to both.
pub fn tile_size_bytes_minus_1_for(non_last_tile_lens: &[usize]) -> u8 {
    let max_tile_size = non_last_tile_lens.iter().copied().max().unwrap_or(0) as u64;
    if (max_tile_size >> 24) != 0 {
        3
    } else if (max_tile_size >> 16) != 0 {
        2
    } else if (max_tile_size >> 8) != 0 {
        1
    } else {
        0
    }
}

/// AV1 spec Section 5.9.15: tile_info().
///
/// Tile COLUMNS stay fixed at a single column (`TileColsLog2 = 0`,
/// unchanged from before task #86 — out of scope). Tile ROWS honor
/// `tile_rows_log2`, which the caller MUST have already resolved via
/// [`resolve_tile_rows_log2`] (this function trusts it and does not
/// re-clamp, so the emitted bits always match how many tiles were
/// actually encoded).
///
/// `tile_size_bytes_minus_1` is only meaningful (and only written, as the
/// spec's trailer `tile_size_bytes_minus_1 f(2)`) when `NumTiles > 1`; the
/// caller derives it from the real per-tile byte lengths via
/// [`tile_size_bytes_minus_1_for`] and must pass the SAME value used to
/// build the tile group (`build_tile_group_multi`) or the FH's declared
/// `TileSizeBytes` will disagree with the tile group's actual size
/// prefixes.
fn write_tile_info(
    wb: &mut BitWriter,
    width: u32,
    height: u32,
    tile_rows_log2: u8,
    tile_cols_log2: u8,
    tile_size_bytes_minus_1: u8,
    sb_size: u32,
) {
    let grid = TileGrid::resolve(width, height, sb_size, tile_rows_log2, tile_cols_log2);
    debug_assert_eq!(
        (grid.tile_rows_log2, grid.tile_cols_log2),
        (tile_rows_log2, tile_cols_log2),
        "tile log2s must be pre-resolved via TileGrid::resolve"
    );

    wb.write_bit(true); // uniform_tile_spacing_flag = 1

    // C write_tile_info_max_tile (entropy_coding.c:2403): for each of
    // columns then rows, `ones` increment bits (each a 1), then — UNLESS
    // the log2 already reached its max — one final 0 stop bit.
    let log2_tile_cols = u32::from(grid.tile_cols_log2);
    for _ in 0..(log2_tile_cols - grid.min_log2_tile_cols) {
        wb.write_bit(true); // increment_tile_cols_log2 = 1 → continue
    }
    if log2_tile_cols < grid.max_log2_tile_cols {
        wb.write_bit(false); // increment_tile_cols_log2 = 0 → stop
    }

    // C recomputes minLog2TileRows against the CHOSEN columns right here
    // (entropy_coding.c:2417) — TileGrid::resolve stores that same value.
    let log2_tile_rows = u32::from(grid.tile_rows_log2);
    for _ in 0..(log2_tile_rows - grid.min_log2_tile_rows) {
        wb.write_bit(true); // increment_tile_rows_log2 = 1 → continue
    }
    if log2_tile_rows < grid.max_log2_tile_rows {
        wb.write_bit(false); // increment_tile_rows_log2 = 0 → stop
    }

    // Spec 5.9.15's `if (TileColsLog2 > 0 || TileRowsLog2 > 0)`, which C
    // spells as `tile_rows * tile_cols > 1` over the ACTUAL counts
    // (entropy_coding.c:2587). The two agree: a nonzero log2 always
    // yields at least 2 tiles on that axis.
    if grid.num_tiles() > 1 {
        // context_update_tile_id: SVT always picks the LAST tile — C
        // writes the literal `tile_cnt - 1` in `log2_tile_cols +
        // log2_tile_rows` bits (entropy_coding.c:2593-2595). `tile_cnt`
        // is the ACTUAL `tile_rows * tile_cols`, which can be less than
        // `1 << (rows_log2 + cols_log2)` — see TileGrid's doc comment.
        // Writing the all-ones pattern instead makes the id exceed
        // NumTiles-1 and the frame is REJECTED by conforming decoders.
        wb.write_bits(grid.num_tiles() as u32 - 1, log2_tile_cols + log2_tile_rows);
        wb.write_bits(u32::from(tile_size_bytes_minus_1), 2);
    }
}

/// C `svt_aom_wb_write_inv_signed_literal` (entropy_coding.c:1377-1379):
/// `svt_aom_wb_write_literal(wb, data, bits + 1)` — i.e. the low `bits + 1`
/// bits of `data`'s two's-complement representation, MSB first. This is the
/// spec's `su(1 + bits)`.
///
/// Kept next to `write_segmentation_params`, its only caller in this port;
/// the delta-Q writer open-codes the same shape inline (`d & 0x7f`, 7 bits).
#[inline]
fn write_inv_signed_literal(wb: &mut BitWriter, data: i32, bits: u32) {
    // `write_bits` only emits the low `n` bits of its argument, so the cast
    // is the mask: for bits = 6 a value of -10 writes 0b1110110 (7 bits).
    wb.write_bits(data as u32, bits + 1);
}

/// `segmentation_params()` — spec 5.9.14, C `encode_segmentation`
/// (entropy_coding.c:2247-2276).
///
/// `primary_ref_none` is `frm_hdr.primary_ref_frame == PRIMARY_REF_NONE`,
/// which is always true for a KEY frame: C only signals the three update
/// flags when a primary reference EXISTS (there is prior state to inherit);
/// with PRIMARY_REF_NONE the decoder infers
/// `update_map = update_data = 1, temporal_update = 0` (spec 5.9.14) and C
/// writes nothing.
///
/// Feature payloads use `svt_aom_segmentation_feature_bits[j]` and the
/// signed/unsigned split from `svt_aom_segmentation_feature_signed[j]`
/// (segmentation_params.c:16-18). Note `bits[SEG_LVL_SKIP] =
/// bits[SEG_LVL_GLOBALMV] = 0`: those are bare enable flags whose "payload"
/// is a zero-length literal, so the enable bit is all that is coded.
///
/// C has a literal `//TODO: add clamping` at :2263 — feature_data is written
/// UNCLAMPED, so a value outside `svt_aom_segmentation_feature_max[j]`
/// silently truncates to the low bits. Reproduced (no clamp) so the port
/// matches C byte-for-byte if a caller ever supplies an out-of-range value;
/// [`svtav1_types::segmentation::SEGMENTATION_FEATURE_MAX`] is transcribed
/// for whenever C adds the clamp.
///
/// # Not wired
///
/// The five in-tree callers still write a hardcoded
/// `segmentation_enabled = 0`. Calling this with a default
/// [`SegmentationParams`] emits exactly that single zero bit, so switching a
/// site over is byte-neutral until a caller actually enables segmentation.
pub fn write_segmentation_params(
    wb: &mut BitWriter,
    seg: &SegmentationParams,
    primary_ref_none: bool,
) {
    wb.write_bit(seg.segmentation_enabled);
    if !seg.segmentation_enabled {
        return;
    }
    if !primary_ref_none {
        wb.write_bit(seg.segmentation_update_map);
        if seg.segmentation_update_map {
            wb.write_bit(seg.segmentation_temporal_update);
        }
        wb.write_bit(seg.segmentation_update_data);
    }
    if seg.segmentation_update_data {
        for i in 0..MAX_SEGMENTS {
            for j in 0..SEG_LVL_MAX {
                let enabled = seg.feature_enabled[i][j] != 0;
                wb.write_bit(enabled);
                if enabled {
                    let data = i32::from(seg.feature_data[i][j]);
                    let bits = SEGMENTATION_FEATURE_BITS[j] as u32;
                    if SEGMENTATION_FEATURE_SIGNED[j] != 0 {
                        write_inv_signed_literal(wb, data, bits);
                    } else {
                        wb.write_bits(data as u32, bits);
                    }
                }
            }
        }
    }
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
/// then for each tile except the last: `TileSizeBytes`-byte little-endian
/// `tile_size_minus_1`, followed by the tile's data. The last tile has no
/// size prefix (C `svt_aom_write_frame_header_av1`,
/// entropy_coding.c:3906-3919, `mem_put_varsize`).
///
/// `tile_size_bytes_minus_1` MUST be [`tile_size_bytes_minus_1_for`] of
/// `tile_bitstreams[..len-1]`'s lengths — computed by the caller (once,
/// alongside the frame header's identical trailer field) rather than
/// re-derived here, so the two can never silently disagree.
pub fn build_tile_group_multi(tile_bitstreams: &[Vec<u8>], tile_size_bytes_minus_1: u8) -> Vec<u8> {
    if tile_bitstreams.len() <= 1 {
        return build_tile_group_single(
            tile_bitstreams.first().map(|v| v.as_slice()).unwrap_or(&[]),
        );
    }
    let tile_size_bytes = tile_size_bytes_minus_1 + 1;
    debug_assert!(
        (1..=4).contains(&tile_size_bytes),
        "tile_size_bytes_minus_1 must be 0..=3"
    );
    debug_assert_eq!(
        tile_size_bytes_minus_1,
        tile_size_bytes_minus_1_for(
            &tile_bitstreams[..tile_bitstreams.len() - 1]
                .iter()
                .map(|t| t.len())
                .collect::<Vec<_>>()
        ),
        "tile_size_bytes_minus_1 must match tile_size_bytes_minus_1_for(non-last tile lengths)"
    );

    let mut wb = BitWriter::new();
    // NumTiles > 1: tile_start_and_end_present_flag = 0 (all tiles in this
    // TG), then byte_alignment() — zero bits, NOT trailing_bits (spec 5.11.1).
    wb.write_bit(false);
    byte_align_zero(&mut wb);
    let header = wb.into_data();

    let total_size: usize = header.len()
        + tile_bitstreams[..tile_bitstreams.len() - 1]
            .iter()
            .map(|t| tile_size_bytes as usize + t.len())
            .sum::<usize>()
        + tile_bitstreams.last().map_or(0, |t| t.len());

    let mut result = Vec::with_capacity(total_size);
    result.extend_from_slice(&header);

    for (i, tile) in tile_bitstreams.iter().enumerate() {
        if i < tile_bitstreams.len() - 1 {
            let size_minus_1 = (tile.len() as u32).saturating_sub(1);
            result.extend_from_slice(&put_varsize(tile_size_bytes, size_minus_1));
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
mod tests;
