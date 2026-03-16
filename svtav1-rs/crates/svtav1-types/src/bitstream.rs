//! AV1 bitstream structure definitions.
//!
//! Ported from `av1_structs.h`.

use crate::block::BlockSize;
use crate::frame::ColorConfig;
use crate::motion::{MAX_PARAMDIM, TransformationType};

/// OBU (Open Bitstream Unit) types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// OBU header.
#[derive(Debug, Clone, Copy, Default)]
pub struct ObuHeader {
    /// Size (1 or 2 bytes) of OBU header.
    pub size: usize,
    /// Must be 0.
    pub obu_forbidden_bit: u8,
    /// Type of data in the OBU payload.
    pub obu_type: u8,
    /// Whether the optional extension header is present.
    pub obu_extension_flag: u8,
    /// Whether obu_size is present.
    pub obu_has_size_field: u8,
    /// Temporal level of the data.
    pub temporal_id: u8,
    /// Spatial level of the data.
    pub spatial_id: u8,
    /// Size of the payload.
    pub payload_size: usize,
}

/// Decoder model information.
#[derive(Debug, Clone, Copy, Default)]
pub struct DecoderModelInfo {
    pub buffer_delay_length_minus_1: u8,
    pub num_units_in_decoding_tick: u32,
    pub buffer_removal_time_length_minus_1: u8,
    pub frame_presentation_time_length_minus_1: u8,
}

/// Order hint information.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrderHintInfo {
    pub enable_order_hint: bool,
    pub enable_jnt_comp: bool,
    pub enable_ref_frame_mvs: bool,
    pub order_hint_bits: u8,
}

/// Timing information.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimingInfo {
    pub timing_info_present: bool,
    pub num_units_in_display_tick: u32,
    pub time_scale: u32,
    pub equal_picture_interval: bool,
    pub num_ticks_per_picture: u32,
}

/// Maximum number of operating points.
pub const MAX_NUM_OPERATING_POINTS: usize = 32;

/// Sequence header.
#[derive(Debug, Clone)]
pub struct SeqHeader {
    /// Profile (0, 1, or 2).
    pub seq_profile: u8,
    /// Whether this is a still picture sequence.
    pub still_picture: bool,
    /// Whether the reduced still picture header is used.
    pub reduced_still_picture_header: bool,
    /// Timing information.
    pub timing_info: TimingInfo,
    /// Whether decoder model info is present.
    pub decoder_model_info_present_flag: bool,
    /// Decoder model information.
    pub decoder_model_info: DecoderModelInfo,
    /// Whether initial display delay is present.
    pub initial_display_delay_present_flag: bool,
    /// Number of operating points minus 1.
    pub operating_points_cnt_minus_1: u8,
    /// Bits for frame width.
    pub frame_width_bits: u8,
    /// Bits for frame height.
    pub frame_height_bits: u8,
    /// Maximum frame width.
    pub max_frame_width: u16,
    /// Maximum frame height.
    pub max_frame_height: u16,
    /// Whether frame ID numbers are present.
    pub frame_id_numbers_present_flag: bool,
    pub delta_frame_id_length: u8,
    pub frame_id_length: u8,
    /// Whether 128x128 superblocks are used.
    pub use_128x128_superblock: bool,
    /// Superblock size.
    pub sb_size: BlockSize,
    /// Superblock size in 4x4 MI units.
    pub sb_mi_size: u8,
    /// Log2 of superblock size.
    pub sb_size_log2: u8,
    /// Filter intra level.
    pub filter_intra_level: u8,
    /// Enable intra edge filter.
    pub enable_intra_edge_filter: bool,
    /// Enable intra angle delta filter.
    pub enable_intra_angle_delta_filter: bool,
    /// Enable interintra compound.
    pub enable_interintra_compound: bool,
    /// Enable masked compound.
    pub enable_masked_compound: bool,
    /// Enable warped motion.
    pub enable_warped_motion: bool,
    /// Enable dual filter (independent H/V interpolation).
    pub enable_dual_filter: bool,
    /// Order hint information.
    pub order_hint_info: OrderHintInfo,
    /// Screen content tools control.
    pub seq_force_screen_content_tools: u8,
    /// Integer MV control.
    pub seq_force_integer_mv: u8,
    /// Enable super-resolution.
    pub enable_superres: bool,
    /// CDEF level.
    pub cdef_level: u8,
    /// Enable loop restoration.
    pub enable_restoration: bool,
    /// Color configuration.
    pub color_config: ColorConfig,
    /// Whether film grain params are present.
    pub film_grain_params_present: bool,
}

/// Frame size information.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameSize {
    pub frame_width: u16,
    pub frame_height: u16,
    pub render_width: u16,
    pub render_height: u16,
    pub superres_denominator: u8,
    pub superres_upscaled_width: u16,
    pub superres_upscaled_height: u16,
}

/// Maximum tile rows and columns.
pub const MAX_TILE_ROWS: usize = 64;
pub const MAX_TILE_COLS: usize = 64;
/// Maximum tile width in pixels.
pub const MAX_TILE_WIDTH: usize = 4096;
/// Maximum tile area in pixels.
pub const MAX_TILE_AREA: usize = 4096 * 2304;

/// Tiles information.
#[derive(Debug, Clone)]
pub struct TilesInfo {
    pub max_tile_width_sb: u16,
    pub max_tile_height_sb: u16,
    pub min_log2_tile_cols: u8,
    pub max_log2_tile_cols: u8,
    pub max_log2_tile_rows: u8,
    pub min_log2_tile_rows: u8,
    pub min_log2_tiles: u8,
    pub uniform_tile_spacing_flag: bool,
    pub tile_cols: u8,
    pub tile_rows: u8,
    pub tile_cols_log2: u8,
    pub tile_rows_log2: u8,
    pub tile_col_start_mi: [u16; MAX_TILE_ROWS + 1],
    pub tile_row_start_mi: [u16; MAX_TILE_COLS + 1],
    pub context_update_tile_id: u16,
    pub tile_size_bytes: u8,
}

/// Delta Q parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeltaQParams {
    pub delta_q_present: bool,
    pub delta_q_res: u8,
}

/// Delta loop filter parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeltaLfParams {
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,
}

/// Loop restoration parameters per plane.
#[derive(Debug, Clone, Copy, Default)]
pub struct LrParams {
    pub frame_restoration_type: u8,
    pub loop_restoration_size: u16,
    pub lr_size_log2: u8,
}

/// Skip mode information.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkipModeInfo {
    pub skip_mode_allowed: bool,
    pub skip_mode_flag: bool,
    pub ref_frame_idx_0: i32,
    pub ref_frame_idx_1: i32,
}

/// Global motion parameters for a reference frame.
#[derive(Debug, Clone, Copy)]
pub struct GlobalMotionParams {
    pub gm_type: TransformationType,
    pub gm_params: [i32; MAX_PARAMDIM],
}

impl Default for GlobalMotionParams {
    fn default() -> Self {
        Self {
            gm_type: TransformationType::Identity,
            gm_params: [0; MAX_PARAMDIM],
        }
    }
}

/// Tile information (per-tile position).
///
/// Ported from `block_structures.h` lines 27-34.
#[derive(Debug, Clone, Copy, Default)]
pub struct TileInfo {
    pub mi_row_start: i32,
    pub mi_row_end: i32,
    pub mi_col_start: i32,
    pub mi_col_end: i32,
    pub tg_horz_boundary: i32,
    pub tile_row: i32,
    pub tile_col: i32,
    /// Tile index in raster order.
    pub tile_rs_index: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obu_type_discriminants() {
        assert_eq!(ObuType::SequenceHeader as u8, 1);
        assert_eq!(ObuType::Padding as u8, 15);
    }

    #[test]
    fn tile_info_default() {
        let ti = TileInfo::default();
        assert_eq!(ti.mi_row_start, 0);
        assert_eq!(ti.tile_rs_index, 0);
    }

    #[test]
    fn global_motion_default() {
        let gm = GlobalMotionParams::default();
        assert_eq!(gm.gm_type, TransformationType::Identity);
        assert_eq!(gm.gm_params, [0; MAX_PARAMDIM]);
    }
}
