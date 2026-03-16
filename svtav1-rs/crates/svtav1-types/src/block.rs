//! Block size definitions.
//!
//! Ported from `definitions.h` lines 829-856.

/// AV1 block sizes. Values match the C enum exactly.
///
/// Note: The first 16 variants (BLOCK_4X4..BLOCK_128X128) are the "standard"
/// block sizes (square + 2:1/1:2 rectangular). The last 6 variants are
/// "extended rectangular" (4:1/1:4 ratios).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum BlockSize {
    Block4x4 = 0,
    Block4x8 = 1,
    Block8x4 = 2,
    Block8x8 = 3,
    Block8x16 = 4,
    Block16x8 = 5,
    Block16x16 = 6,
    Block16x32 = 7,
    Block32x16 = 8,
    Block32x32 = 9,
    Block32x64 = 10,
    Block64x32 = 11,
    Block64x64 = 12,
    Block64x128 = 13,
    Block128x64 = 14,
    Block128x128 = 15,
    // Extended rectangular (4:1 / 1:4)
    Block4x16 = 16,
    Block16x4 = 17,
    Block8x32 = 18,
    Block32x8 = 19,
    Block16x64 = 20,
    Block64x16 = 21,
}

impl BlockSize {
    /// Total number of block size variants (BLOCK_SIZES_ALL).
    pub const SIZES_ALL: usize = 22;

    /// Number of "standard" block sizes (square + 2:1/1:2).
    pub const SIZES: usize = 16;

    /// The largest standard block size (BLOCK_128X128).
    pub const LARGEST: Self = Self::Block128x128;

    /// Invalid sentinel value matching C's BLOCK_INVALID = 255.
    pub const INVALID_RAW: u8 = 255;

    /// Maximum number of blocks that can be allocated per superblock.
    pub const MAX_NUM_BLOCKS_ALLOC: usize = 4421;

    /// Convert from raw u8 discriminant. Returns None for out-of-range values.
    #[inline]
    pub const fn from_u8(v: u8) -> Option<Self> {
        if v < Self::SIZES_ALL as u8 {
            // SAFETY: all values 0..22 are valid discriminants
            // But we avoid unsafe — use match instead
            Some(match v {
                0 => Self::Block4x4,
                1 => Self::Block4x8,
                2 => Self::Block8x4,
                3 => Self::Block8x8,
                4 => Self::Block8x16,
                5 => Self::Block16x8,
                6 => Self::Block16x16,
                7 => Self::Block16x32,
                8 => Self::Block32x16,
                9 => Self::Block32x32,
                10 => Self::Block32x64,
                11 => Self::Block64x32,
                12 => Self::Block64x64,
                13 => Self::Block64x128,
                14 => Self::Block128x64,
                15 => Self::Block128x128,
                16 => Self::Block4x16,
                17 => Self::Block16x4,
                18 => Self::Block8x32,
                19 => Self::Block32x8,
                20 => Self::Block16x64,
                21 => Self::Block64x16,
                _ => unreachable!(),
            })
        } else {
            None
        }
    }

    /// Index into lookup tables (same as discriminant value).
    #[inline]
    pub const fn as_index(self) -> usize {
        self as usize
    }

    /// Returns true if this is a square block size.
    #[inline]
    pub const fn is_square(self) -> bool {
        matches!(
            self,
            Self::Block4x4
                | Self::Block8x8
                | Self::Block16x16
                | Self::Block32x32
                | Self::Block64x64
                | Self::Block128x128
        )
    }

    /// Iterator over all block sizes.
    pub const ALL: [Self; Self::SIZES_ALL] = [
        Self::Block4x4,
        Self::Block4x8,
        Self::Block8x4,
        Self::Block8x8,
        Self::Block8x16,
        Self::Block16x8,
        Self::Block16x16,
        Self::Block16x32,
        Self::Block32x16,
        Self::Block32x32,
        Self::Block32x64,
        Self::Block64x32,
        Self::Block64x64,
        Self::Block64x128,
        Self::Block128x64,
        Self::Block128x128,
        Self::Block4x16,
        Self::Block16x4,
        Self::Block8x32,
        Self::Block32x8,
        Self::Block16x64,
        Self::Block64x16,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_size_discriminants_match_c() {
        assert_eq!(BlockSize::Block4x4 as u8, 0);
        assert_eq!(BlockSize::Block128x128 as u8, 15);
        assert_eq!(BlockSize::Block4x16 as u8, 16);
        assert_eq!(BlockSize::Block64x16 as u8, 21);
    }

    #[test]
    fn block_size_roundtrip() {
        for bs in BlockSize::ALL {
            assert_eq!(BlockSize::from_u8(bs as u8), Some(bs));
        }
        assert_eq!(BlockSize::from_u8(22), None);
        assert_eq!(BlockSize::from_u8(255), None);
    }

    #[test]
    fn block_size_square() {
        let squares: Vec<_> = BlockSize::ALL.iter().filter(|b| b.is_square()).collect();
        assert_eq!(squares.len(), 6);
    }
}
