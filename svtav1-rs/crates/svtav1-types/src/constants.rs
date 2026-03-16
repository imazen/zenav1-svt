//! Miscellaneous constants used across the encoder.
//!
//! Ported from various locations in `definitions.h`.

/// Block size 64 pixels.
pub const BLOCK_SIZE_64: u32 = 64;
/// Log2 of minimum block size (8 pixels).
pub const LOG_MIN_BLOCK_SIZE: u32 = 3;
/// Minimum block size in pixels.
pub const MIN_BLOCK_SIZE: u32 = 1 << LOG_MIN_BLOCK_SIZE;

/// Maximum temporal layers.
pub const MAX_TEMPORAL_LAYERS: usize = 6;
/// Maximum number of reference picture lists.
pub const MAX_NUM_OF_REF_PIC_LIST: usize = 2;
/// Maximum reference index.
pub const MAX_REF_IDX: usize = 4;

/// HME search area limits.
pub const EB_HME_SEARCH_AREA_COLUMN_MAX_COUNT: usize = 2;
pub const EB_HME_SEARCH_AREA_ROW_MAX_COUNT: usize = 2;

/// Maximum TPL group size.
pub const MAX_TPL_GROUP_SIZE: usize = 512;

/// Maximum tile count (Annex A.3).
pub const MAX_TILE_CNTS: usize = 128;

/// Maximum number of operating points.
pub const MAX_NUM_TEMPORAL_LAYERS_AV1: usize = 8;
pub const MAX_NUM_SPATIAL_LAYERS: usize = 4;
pub const MAX_NUM_OPERATING_POINTS: usize = MAX_NUM_TEMPORAL_LAYERS_AV1 * MAX_NUM_SPATIAL_LAYERS;

/// Primary reference constants.
pub const PRIMARY_REF_BITS: usize = 3;
pub const PRIMARY_REF_NONE: usize = 7;

/// Invalid buffer index.
pub const INVALID_IDX: i32 = -1;

/// ME filter tap count.
pub const ME_FILTER_TAP: usize = 4;

/// Maximum number of blocks allocatable per superblock.
pub const MAX_NUM_BLOCKS_ALLOC: usize = 4421;

/// Bitstream buffer size estimate for a given number of pixels (assuming 4:2:0).
#[inline]
pub const fn bitstream_buffer_size(pixels: usize) -> usize {
    pixels * 3 / 2 * 2
}
