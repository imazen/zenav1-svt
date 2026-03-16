//! Block size lookup tables.
use svtav1_types::block::BlockSize;

/// Width in pixels for each BlockSize variant.
pub const BLOCK_SIZE_WIDE: [u8; BlockSize::SIZES_ALL] = [
    4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 32, 64, 64, 64, 128, 128, // standard
    4, 16, 8, 32, 16, 64, // extended rectangular
];

/// Height in pixels for each BlockSize variant.
pub const BLOCK_SIZE_HIGH: [u8; BlockSize::SIZES_ALL] = [
    4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 64, 32, 64, 128, 64, 128, // standard
    16, 4, 32, 8, 64, 16, // extended rectangular
];

/// Log2 of width for each BlockSize variant.
pub const BLOCK_SIZE_WIDE_LOG2: [u8; BlockSize::SIZES_ALL] = [
    2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 5, 6, 6, 6, 7, 7, // standard
    2, 4, 3, 5, 4, 6, // extended rectangular
];

/// Log2 of height for each BlockSize variant.
pub const BLOCK_SIZE_HIGH_LOG2: [u8; BlockSize::SIZES_ALL] = [
    2, 3, 2, 3, 4, 3, 4, 5, 4, 5, 6, 5, 6, 7, 6, 7, // standard
    4, 2, 5, 3, 6, 4, // extended rectangular
];

/// Number of 4x4 units wide for each BlockSize.
pub const NUM_4X4_BLOCKS_WIDE: [u8; BlockSize::SIZES_ALL] = [
    1, 1, 2, 2, 2, 4, 4, 4, 8, 8, 8, 16, 16, 16, 32, 32, 1, 4, 2, 8, 4, 16,
];

/// Number of 4x4 units high for each BlockSize.
pub const NUM_4X4_BLOCKS_HIGH: [u8; BlockSize::SIZES_ALL] = [
    1, 2, 1, 2, 4, 2, 4, 8, 4, 8, 16, 8, 16, 32, 16, 32, 4, 1, 8, 2, 16, 4,
];

/// Partition context: (above, left) for each BlockSize.
pub const PARTITION_CONTEXT_LOOKUP: [(i8, i8); BlockSize::SIZES_ALL] = [
    (31, 31), // 4X4
    (31, 30), // 4X8
    (30, 31), // 8X4
    (30, 30), // 8X8
    (30, 28), // 8X16
    (28, 30), // 16X8
    (28, 28), // 16X16
    (28, 24), // 16X32
    (24, 28), // 32X16
    (24, 24), // 32X32
    (24, 16), // 32X64
    (16, 24), // 64X32
    (16, 16), // 64X64
    (16, 0),  // 64X128
    (0, 16),  // 128X64
    (0, 0),   // 128X128
    (31, 28), // 4X16
    (28, 31), // 16X4
    (30, 24), // 8X32
    (24, 30), // 32X8
    (28, 16), // 16X64
    (16, 28), // 64X16
];
