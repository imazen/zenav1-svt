//! Transform-related lookup tables.
use svtav1_types::transform::{TxSize, TxType, TxType1D};

/// Mapping from 2D TxType to (column 1D type, row 1D type).
pub const TX_TYPE_TO_1D: [(TxType1D, TxType1D); TxType::COUNT] = [
    (TxType1D::Dct, TxType1D::Dct),           // DCT_DCT
    (TxType1D::Adst, TxType1D::Dct),          // ADST_DCT
    (TxType1D::Dct, TxType1D::Adst),          // DCT_ADST
    (TxType1D::Adst, TxType1D::Adst),         // ADST_ADST
    (TxType1D::FlipAdst, TxType1D::Dct),      // FLIPADST_DCT
    (TxType1D::Dct, TxType1D::FlipAdst),      // DCT_FLIPADST
    (TxType1D::FlipAdst, TxType1D::FlipAdst), // FLIPADST_FLIPADST
    (TxType1D::Adst, TxType1D::FlipAdst),     // ADST_FLIPADST
    (TxType1D::FlipAdst, TxType1D::Adst),     // FLIPADST_ADST
    (TxType1D::Identity, TxType1D::Identity), // IDTX
    (TxType1D::Dct, TxType1D::Identity),      // V_DCT
    (TxType1D::Identity, TxType1D::Dct),      // H_DCT
    (TxType1D::Adst, TxType1D::Identity),     // V_ADST
    (TxType1D::Identity, TxType1D::Adst),     // H_ADST
    (TxType1D::FlipAdst, TxType1D::Identity), // V_FLIPADST
    (TxType1D::Identity, TxType1D::FlipAdst), // H_FLIPADST
];

/// Log2 of width for each TxSize variant.
pub const TX_SIZE_WIDE_LOG2: [u8; TxSize::SIZES_ALL] = [
    2, 3, 4, 5, 6, // square: 4, 8, 16, 32, 64
    2, 3, 3, 4, 4, 5, 5, 6, // rectangular 2:1/1:2
    2, 4, 3, 5, 4, 6, // extended 4:1/1:4
];

/// Log2 of height for each TxSize variant.
pub const TX_SIZE_HIGH_LOG2: [u8; TxSize::SIZES_ALL] = [
    2, 3, 4, 5, 6, // square
    3, 2, 4, 3, 5, 4, 6, 5, // rectangular
    4, 2, 5, 3, 6, 4, // extended
];

/// Width in pixels for each TxSize variant.
pub const TX_SIZE_WIDE: [u8; TxSize::SIZES_ALL] = [
    4, 8, 16, 32, 64, 4, 8, 8, 16, 16, 32, 32, 64, 4, 16, 8, 32, 16, 64,
];

/// Height in pixels for each TxSize variant.
pub const TX_SIZE_HIGH: [u8; TxSize::SIZES_ALL] = [
    4, 8, 16, 32, 64, 8, 4, 16, 8, 32, 16, 64, 32, 16, 4, 32, 8, 64, 16,
];
