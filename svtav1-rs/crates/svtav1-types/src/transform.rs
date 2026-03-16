//! Transform type and size definitions.
//!
//! Ported from `definitions.h` lines 900-1059.

/// AV1 transform sizes. 5 square + 8 rectangular + 6 extended = 19 total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TxSize {
    Tx4x4 = 0,
    Tx8x8 = 1,
    Tx16x16 = 2,
    Tx32x32 = 3,
    Tx64x64 = 4,
    // Rectangular (2:1 / 1:2)
    Tx4x8 = 5,
    Tx8x4 = 6,
    Tx8x16 = 7,
    Tx16x8 = 8,
    Tx16x32 = 9,
    Tx32x16 = 10,
    Tx32x64 = 11,
    Tx64x32 = 12,
    // Extended rectangular (4:1 / 1:4)
    Tx4x16 = 13,
    Tx16x4 = 14,
    Tx8x32 = 15,
    Tx32x8 = 16,
    Tx16x64 = 17,
    Tx64x16 = 18,
}

impl TxSize {
    /// Total number of transform sizes (TX_SIZES_ALL).
    pub const SIZES_ALL: usize = 19;

    /// Number of square-only transform sizes (TX_SIZES).
    pub const SIZES_SQUARE: usize = 5;

    /// Largest transform size.
    pub const LARGEST: Self = Self::Tx64x64;

    /// Invalid sentinel.
    pub const INVALID_RAW: u8 = 255;

    /// Convert from raw u8.
    #[inline]
    pub const fn from_u8(v: u8) -> Option<Self> {
        if v < Self::SIZES_ALL as u8 {
            Some(match v {
                0 => Self::Tx4x4,
                1 => Self::Tx8x8,
                2 => Self::Tx16x16,
                3 => Self::Tx32x32,
                4 => Self::Tx64x64,
                5 => Self::Tx4x8,
                6 => Self::Tx8x4,
                7 => Self::Tx8x16,
                8 => Self::Tx16x8,
                9 => Self::Tx16x32,
                10 => Self::Tx32x16,
                11 => Self::Tx32x64,
                12 => Self::Tx64x32,
                13 => Self::Tx4x16,
                14 => Self::Tx16x4,
                15 => Self::Tx8x32,
                16 => Self::Tx32x8,
                17 => Self::Tx16x64,
                18 => Self::Tx64x16,
                _ => unreachable!(),
            })
        } else {
            None
        }
    }

    #[inline]
    pub const fn as_index(self) -> usize {
        self as usize
    }
}

/// 2D transform types (16 types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TxType {
    DctDct = 0,
    AdstDct = 1,
    DctAdst = 2,
    AdstAdst = 3,
    FlipAdstDct = 4,
    DctFlipAdst = 5,
    FlipAdstFlipAdst = 6,
    AdstFlipAdst = 7,
    FlipAdstAdst = 8,
    Idtx = 9,
    VDct = 10,
    HDct = 11,
    VAdst = 12,
    HAdst = 13,
    VFlipAdst = 14,
    HFlipAdst = 15,
}

impl TxType {
    /// Number of transform types (TX_TYPES).
    pub const COUNT: usize = 16;

    /// Invalid sentinel matching C's INVALID_TX_TYPE = 16.
    pub const INVALID_RAW: u8 = 16;

    /// Returns true if this is a 2D transform (not identity or 1D).
    #[inline]
    pub const fn is_2d(self) -> bool {
        (self as u8) < Self::Idtx as u8
    }
}

/// 1D transform types used as building blocks for 2D transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TxType1D {
    Dct = 0,
    Adst = 1,
    FlipAdst = 2,
    Identity = 3,
}

impl TxType1D {
    pub const COUNT: usize = 4;
}

/// Transform class (2D, horizontal, vertical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TxClass {
    TwoD = 0,
    Horiz = 1,
    Vert = 2,
}

impl TxClass {
    pub const COUNT: usize = 3;
}

/// Frame transform mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TxMode {
    /// Use only 4x4 transform.
    Only4x4 = 0,
    /// Transform size is the largest possible for PU size.
    Largest = 1,
    /// Transform specified for each block.
    Select = 2,
}

impl TxMode {
    pub const COUNT: usize = 3;
}

/// Extended transform set types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TxSetType {
    /// DCT only.
    DctOnly = 0,
    /// DCT + Identity only.
    DctIdtx = 1,
    /// Discrete trig transforms w/o flip (4) + Identity (1).
    Dtt4Idtx = 2,
    /// DTT4 + Identity + 1D Hor/Vert DCT (2).
    Dtt4Idtx1dDct = 3,
    /// DTT9 (with flip) + Identity + 1D Hor/Vert DCT (2).
    Dtt9Idtx1dDct = 4,
    /// All 16 transform types.
    All16 = 5,
}

impl TxSetType {
    pub const COUNT: usize = 6;
}

/// Transform parameters for forward/inverse transforms.
#[derive(Debug, Clone, Copy)]
pub struct TxfmParam {
    pub tx_type: TxType,
    pub tx_size: TxSize,
    pub lossless: bool,
    pub bd: u8,
    pub is_hbd: bool,
    pub tx_set_type: TxSetType,
    /// For inverse transforms only: end of block position.
    pub eob: i32,
}

/// Number of sizes that use extended transforms.
pub const EXT_TX_SIZES: usize = 4;
/// Sets of transform selections for INTER.
pub const EXT_TX_SETS_INTER: usize = 4;
/// Sets of transform selections for INTRA.
pub const EXT_TX_SETS_INTRA: usize = 3;

/// Transform coefficient type (TranLow in C).
pub type TranLow = i32;

/// Quantization matrix value type.
pub type QmVal = u8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_size_discriminants_match_c() {
        assert_eq!(TxSize::Tx4x4 as u8, 0);
        assert_eq!(TxSize::Tx64x64 as u8, 4);
        assert_eq!(TxSize::Tx4x8 as u8, 5);
        assert_eq!(TxSize::Tx64x16 as u8, 18);
    }

    #[test]
    fn tx_type_discriminants_match_c() {
        assert_eq!(TxType::DctDct as u8, 0);
        assert_eq!(TxType::Idtx as u8, 9);
        assert_eq!(TxType::HFlipAdst as u8, 15);
    }

    #[test]
    fn tx_type_is_2d() {
        assert!(TxType::DctDct.is_2d());
        assert!(TxType::AdstAdst.is_2d());
        assert!(!TxType::Idtx.is_2d());
        assert!(!TxType::VDct.is_2d());
    }

    #[test]
    fn tx_size_roundtrip() {
        for i in 0..TxSize::SIZES_ALL as u8 {
            assert!(TxSize::from_u8(i).is_some());
        }
        assert_eq!(TxSize::from_u8(19), None);
    }
}
