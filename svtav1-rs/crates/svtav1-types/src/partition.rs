//! Partition type definitions.
//!
//! Ported from `definitions.h` lines 858-893.

/// AV1 partition types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PartitionType {
    None = 0,
    Horz = 1,
    Vert = 2,
    Split = 3,
    /// HORZ split and the top partition is split again.
    HorzA = 4,
    /// HORZ split and the bottom partition is split again.
    HorzB = 5,
    /// VERT split and the left partition is split again.
    VertA = 6,
    /// VERT split and the right partition is split again.
    VertB = 7,
    /// 4:1 horizontal partition.
    Horz4 = 8,
    /// 4:1 vertical partition.
    Vert4 = 9,
}

impl PartitionType {
    /// Number of extended partition types (EXT_PARTITION_TYPES).
    pub const EXT_TYPES: usize = 10;

    /// Number of basic partition types (PARTITION_TYPES = SPLIT + 1).
    pub const BASIC_TYPES: usize = 4;

    /// Invalid sentinel.
    pub const INVALID_RAW: u8 = 255;
}

/// Internal encoder partition shape.
///
/// Ported from `definitions.h` lines 876-887.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Part {
    N = 0,
    H = 1,
    V = 2,
    H4 = 3,
    V4 = 4,
    Ha = 5,
    Hb = 6,
    Va = 7,
    Vb = 8,
    S = 9,
}

/// Partition context offsets.
pub const PARTITION_PLOFFSET: usize = 4;
pub const PARTITION_BLOCK_SIZES: usize = 5;
pub const PARTITION_CONTEXTS: usize = PARTITION_BLOCK_SIZES * PARTITION_PLOFFSET;
