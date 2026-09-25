use super::*;

/// Port of C `svt_av1_fadst16_new_N2` (transforms.c).
pub fn fadst16_n2(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 16];
    // stage 0;
    // stage 1;
    output[0] = input[0];
    output[1] = -input[15];
    output[2] = -input[7];
    output[3] = input[8];
    output[4] = -input[3];
    output[5] = input[12];
    output[6] = input[4];
    output[7] = -input[11];
    output[8] = -input[1];
    output[9] = input[14];
    output[10] = input[6];
    output[11] = -input[9];
    output[12] = input[2];
    output[13] = -input[13];
    output[14] = -input[5];
    output[15] = input[10];
    // stage 2
    step[0] = output[0];
    step[1] = output[1];
    step[2] = half_btf(cospi[32], output[2], cospi[32], output[3], cos_bit as u32);
    step[3] = half_btf(cospi[32], output[2], -cospi[32], output[3], cos_bit as u32);
    step[4] = output[4];
    step[5] = output[5];
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[32], output[6], -cospi[32], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(cospi[32], output[10], cospi[32], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[32],
        output[10],
        -cospi[32],
        output[11],
        cos_bit as u32,
    );
    step[12] = output[12];
    step[13] = output[13];
    step[14] = half_btf(cospi[32], output[14], cospi[32], output[15], cos_bit as u32);
    step[15] = half_btf(
        cospi[32],
        output[14],
        -cospi[32],
        output[15],
        cos_bit as u32,
    );
    // stage 3
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];
    output[8] = step[8] + step[10];
    output[9] = step[9] + step[11];
    output[10] = step[8] - step[10];
    output[11] = step[9] - step[11];
    output[12] = step[12] + step[14];
    output[13] = step[13] + step[15];
    output[14] = step[12] - step[14];
    output[15] = step[13] - step[15];
    // stage 4
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[16], output[4], cospi[48], output[5], cos_bit as u32);
    step[5] = half_btf(cospi[48], output[4], -cospi[16], output[5], cos_bit as u32);
    step[6] = half_btf(-cospi[48], output[6], cospi[16], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[16], output[6], cospi[48], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = output[10];
    step[11] = output[11];
    step[12] = half_btf(cospi[16], output[12], cospi[48], output[13], cos_bit as u32);
    step[13] = half_btf(
        cospi[48],
        output[12],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[14] = half_btf(
        -cospi[48],
        output[14],
        cospi[16],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[16], output[14], cospi[48], output[15], cos_bit as u32);
    // stage 5
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[2] = step[2] + step[6];
    output[3] = step[3] + step[7];
    output[4] = step[0] - step[4];
    output[5] = step[1] - step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];
    output[8] = step[8] + step[12];
    output[9] = step[9] + step[13];
    output[10] = step[10] + step[14];
    output[11] = step[11] + step[15];
    output[12] = step[8] - step[12];
    output[13] = step[9] - step[13];
    output[14] = step[10] - step[14];
    output[15] = step[11] - step[15];
    // stage 6
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = output[4];
    step[5] = output[5];
    step[6] = output[6];
    step[7] = output[7];
    step[8] = half_btf(cospi[8], output[8], cospi[56], output[9], cos_bit as u32);
    step[9] = half_btf(cospi[56], output[8], -cospi[8], output[9], cos_bit as u32);
    step[10] = half_btf(cospi[40], output[10], cospi[24], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[24],
        output[10],
        -cospi[40],
        output[11],
        cos_bit as u32,
    );
    step[12] = half_btf(-cospi[56], output[12], cospi[8], output[13], cos_bit as u32);
    step[13] = half_btf(cospi[8], output[12], cospi[56], output[13], cos_bit as u32);
    step[14] = half_btf(
        -cospi[24],
        output[14],
        cospi[40],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[40], output[14], cospi[24], output[15], cos_bit as u32);
    // stage 7
    output[0] = step[0] + step[8];
    output[1] = step[1] + step[9];
    output[2] = step[2] + step[10];
    output[3] = step[3] + step[11];
    output[4] = step[4] + step[12];
    output[5] = step[5] + step[13];
    output[6] = step[6] + step[14];
    output[7] = step[7] + step[15];
    output[8] = step[0] - step[8];
    output[9] = step[1] - step[9];
    output[10] = step[2] - step[10];
    output[11] = step[3] - step[11];
    output[12] = step[4] - step[12];
    output[13] = step[5] - step[13];
    output[14] = step[6] - step[14];
    output[15] = step[7] - step[15];
    // stage 8
    step[1] = half_btf(cospi[62], output[0], -cospi[2], output[1], cos_bit as u32);
    step[3] = half_btf(cospi[54], output[2], -cospi[10], output[3], cos_bit as u32);
    step[5] = half_btf(cospi[46], output[4], -cospi[18], output[5], cos_bit as u32);
    step[7] = half_btf(cospi[38], output[6], -cospi[26], output[7], cos_bit as u32);
    step[8] = half_btf(cospi[34], output[8], cospi[30], output[9], cos_bit as u32);
    step[10] = half_btf(cospi[42], output[10], cospi[22], output[11], cos_bit as u32);
    step[12] = half_btf(cospi[50], output[12], cospi[14], output[13], cos_bit as u32);
    step[14] = half_btf(cospi[58], output[14], cospi[6], output[15], cos_bit as u32);
    // stage 9
    output[0] = step[1];
    output[1] = step[14];
    output[2] = step[3];
    output[3] = step[12];
    output[4] = step[5];
    output[5] = step[10];
    output[6] = step[7];
    output[7] = step[8];
}

/// Port of C `svt_av1_fadst16_new_N4` (transforms.c).
pub fn fadst16_n4(input: &[i32], output: &mut [i32], cos_bit: i8) {
    let cospi = cospi_arr(cos_bit);
    let step = &mut [0i32; 16];
    // stage 0;
    // stage 1;
    output[0] = input[0];
    output[1] = -input[15];
    output[2] = -input[7];
    output[3] = input[8];
    output[4] = -input[3];
    output[5] = input[12];
    output[6] = input[4];
    output[7] = -input[11];
    output[8] = -input[1];
    output[9] = input[14];
    output[10] = input[6];
    output[11] = -input[9];
    output[12] = input[2];
    output[13] = -input[13];
    output[14] = -input[5];
    output[15] = input[10];
    // stage 2
    step[0] = output[0];
    step[1] = output[1];
    step[2] = half_btf(cospi[32], output[2], cospi[32], output[3], cos_bit as u32);
    step[3] = half_btf(cospi[32], output[2], -cospi[32], output[3], cos_bit as u32);
    step[4] = output[4];
    step[5] = output[5];
    step[6] = half_btf(cospi[32], output[6], cospi[32], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[32], output[6], -cospi[32], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = half_btf(cospi[32], output[10], cospi[32], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[32],
        output[10],
        -cospi[32],
        output[11],
        cos_bit as u32,
    );
    step[12] = output[12];
    step[13] = output[13];
    step[14] = half_btf(cospi[32], output[14], cospi[32], output[15], cos_bit as u32);
    step[15] = half_btf(
        cospi[32],
        output[14],
        -cospi[32],
        output[15],
        cos_bit as u32,
    );
    // stage 3
    output[0] = step[0] + step[2];
    output[1] = step[1] + step[3];
    output[2] = step[0] - step[2];
    output[3] = step[1] - step[3];
    output[4] = step[4] + step[6];
    output[5] = step[5] + step[7];
    output[6] = step[4] - step[6];
    output[7] = step[5] - step[7];
    output[8] = step[8] + step[10];
    output[9] = step[9] + step[11];
    output[10] = step[8] - step[10];
    output[11] = step[9] - step[11];
    output[12] = step[12] + step[14];
    output[13] = step[13] + step[15];
    output[14] = step[12] - step[14];
    output[15] = step[13] - step[15];
    // stage 4
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = half_btf(cospi[16], output[4], cospi[48], output[5], cos_bit as u32);
    step[5] = half_btf(cospi[48], output[4], -cospi[16], output[5], cos_bit as u32);
    step[6] = half_btf(-cospi[48], output[6], cospi[16], output[7], cos_bit as u32);
    step[7] = half_btf(cospi[16], output[6], cospi[48], output[7], cos_bit as u32);
    step[8] = output[8];
    step[9] = output[9];
    step[10] = output[10];
    step[11] = output[11];
    step[12] = half_btf(cospi[16], output[12], cospi[48], output[13], cos_bit as u32);
    step[13] = half_btf(
        cospi[48],
        output[12],
        -cospi[16],
        output[13],
        cos_bit as u32,
    );
    step[14] = half_btf(
        -cospi[48],
        output[14],
        cospi[16],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[16], output[14], cospi[48], output[15], cos_bit as u32);
    // stage 5
    output[0] = step[0] + step[4];
    output[1] = step[1] + step[5];
    output[2] = step[2] + step[6];
    output[3] = step[3] + step[7];
    output[4] = step[0] - step[4];
    output[5] = step[1] - step[5];
    output[6] = step[2] - step[6];
    output[7] = step[3] - step[7];
    output[8] = step[8] + step[12];
    output[9] = step[9] + step[13];
    output[10] = step[10] + step[14];
    output[11] = step[11] + step[15];
    output[12] = step[8] - step[12];
    output[13] = step[9] - step[13];
    output[14] = step[10] - step[14];
    output[15] = step[11] - step[15];
    // stage 6
    step[0] = output[0];
    step[1] = output[1];
    step[2] = output[2];
    step[3] = output[3];
    step[4] = output[4];
    step[5] = output[5];
    step[6] = output[6];
    step[7] = output[7];
    step[8] = half_btf(cospi[8], output[8], cospi[56], output[9], cos_bit as u32);
    step[9] = half_btf(cospi[56], output[8], -cospi[8], output[9], cos_bit as u32);
    step[10] = half_btf(cospi[40], output[10], cospi[24], output[11], cos_bit as u32);
    step[11] = half_btf(
        cospi[24],
        output[10],
        -cospi[40],
        output[11],
        cos_bit as u32,
    );
    step[12] = half_btf(-cospi[56], output[12], cospi[8], output[13], cos_bit as u32);
    step[13] = half_btf(cospi[8], output[12], cospi[56], output[13], cos_bit as u32);
    step[14] = half_btf(
        -cospi[24],
        output[14],
        cospi[40],
        output[15],
        cos_bit as u32,
    );
    step[15] = half_btf(cospi[40], output[14], cospi[24], output[15], cos_bit as u32);
    // stage 7
    output[0] = step[0] + step[8];
    output[1] = step[1] + step[9];
    output[2] = step[2] + step[10];
    output[3] = step[3] + step[11];
    output[12] = step[4] - step[12];
    output[13] = step[5] - step[13];
    output[14] = step[6] - step[14];
    output[15] = step[7] - step[15];
    // stage 8
    step[1] = half_btf(cospi[62], output[0], -cospi[2], output[1], cos_bit as u32);
    step[3] = half_btf(cospi[54], output[2], -cospi[10], output[3], cos_bit as u32);
    step[12] = half_btf(cospi[50], output[12], cospi[14], output[13], cos_bit as u32);
    step[14] = half_btf(cospi[58], output[14], cospi[6], output[15], cos_bit as u32);
    // stage 9
    output[0] = step[1];
    output[1] = step[14];
    output[2] = step[3];
    output[3] = step[12];
}

// =============================================================================
// Transform configuration — port of `svt_aom_transform_config`
// (transforms.c:3074), `set_fwd_txfm_non_scale_range` (:3051) and
// `svt_av1_gen_fwd_stage_range` (:733), with the tables they read.
//
// The port already carries this logic, but SPLIT and re-transcribed across
// `txfm_dispatch::{flip_cfg, tx_type_to_1d, tx_size_dims}` and
// `fwd_txfm::{fwd_txfm_shift, FWD_COS_BIT_COL, FWD_COS_BIT_ROW}` with no
// binding to the real symbol. This is one struct that a single tier-1
// differential covers over all 16 tx_types x 19 tx_sizes.
// =============================================================================

/// C `MAX_TXFM_STAGE_NUM` (inv_transforms.h:25).
pub const MAX_TXFM_STAGE_NUM: usize = 12;

/// C `TxfmType` (inv_transforms.h:84). `Invalid` is C's `TXFM_TYPE_INVALID`,
/// which `av1_txfm_type_ls` yields for ADST at the 64-point row/column.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TxfmType {
    Dct4 = 0,
    Dct8 = 1,
    Dct16 = 2,
    Dct32 = 3,
    Dct64 = 4,
    Adst4 = 5,
    Adst8 = 6,
    Adst16 = 7,
    Adst32 = 8,
    Identity4 = 9,
    Identity8 = 10,
    Identity16 = 11,
    Identity32 = 12,
    Identity64 = 13,
    /// C `TXFM_TYPE_INVALID` (== `TXFM_TYPES + 1` == 15).
    Invalid = 15,
}

/// C `av1_txfm_type_ls[5][TX_TYPES_1D]` (inv_transforms.h:191), indexed by
/// `[txwh_idx][tx_type_1d]` with `tx_type_1d` in DCT/ADST/FLIPADST/IDTX order.
pub(super) const AV1_TXFM_TYPE_LS: [[TxfmType; 4]; 5] = [
    [
        TxfmType::Dct4,
        TxfmType::Adst4,
        TxfmType::Adst4,
        TxfmType::Identity4,
    ],
    [
        TxfmType::Dct8,
        TxfmType::Adst8,
        TxfmType::Adst8,
        TxfmType::Identity8,
    ],
    [
        TxfmType::Dct16,
        TxfmType::Adst16,
        TxfmType::Adst16,
        TxfmType::Identity16,
    ],
    [
        TxfmType::Dct32,
        TxfmType::Adst32,
        TxfmType::Adst32,
        TxfmType::Identity32,
    ],
    [
        TxfmType::Dct64,
        TxfmType::Invalid,
        TxfmType::Invalid,
        TxfmType::Identity64,
    ],
];

/// C `av1_txfm_stage_num_list[TXFM_TYPES]` (inv_transforms.h:197).
pub(super) const AV1_TXFM_STAGE_NUM_LIST: [i8; 14] = [4, 6, 8, 10, 12, 7, 8, 10, 12, 1, 1, 1, 1, 1];

/// C `fwd_txfm_range_mult2_list[TXFM_TYPES]` (transforms.c:687), flattened
/// into a fixed 12-entry row per type (only the first `stage_num` are read).
pub(super) const FWD_TXFM_RANGE_MULT2_LIST: [[i8; MAX_TXFM_STAGE_NUM]; 14] = [
    [0, 2, 3, 3, 0, 0, 0, 0, 0, 0, 0, 0],        // fdct4
    [0, 2, 4, 5, 5, 5, 0, 0, 0, 0, 0, 0],        // fdct8
    [0, 2, 4, 6, 7, 7, 7, 7, 0, 0, 0, 0],        // fdct16
    [0, 2, 4, 6, 8, 9, 9, 9, 9, 9, 0, 0],        // fdct32
    [0, 2, 4, 6, 8, 10, 11, 11, 11, 11, 11, 11], // fdct64
    [0, 2, 4, 3, 3, 3, 3, 0, 0, 0, 0, 0],        // fadst4
    [0, 0, 1, 3, 3, 5, 5, 5, 0, 0, 0, 0],        // fadst8
    [0, 0, 1, 3, 3, 5, 5, 7, 7, 7, 0, 0],        // fadst16
    [0, 0, 1, 3, 3, 5, 5, 7, 7, 9, 9, 9],        // fadst32
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx4
    [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx8
    [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx16
    [4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx32
    [5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],        // fidtx64
];

/// C `fwd_txfm_shift_ls[TX_SIZES_ALL]` (transforms.c:722), in `TxSize` order.
pub(super) const FWD_TXFM_SHIFT_LS: [[i8; 3]; 19] = [
    [2, 0, 0],   // TX_4X4
    [2, -1, 0],  // TX_8X8
    [2, -2, 0],  // TX_16X16
    [2, -4, 0],  // TX_32X32
    [0, -2, -2], // TX_64X64
    [2, -1, 0],  // TX_4X8
    [2, -1, 0],  // TX_8X4
    [2, -2, 0],  // TX_8X16
    [2, -2, 0],  // TX_16X8
    [2, -4, 0],  // TX_16X32
    [2, -4, 0],  // TX_32X16
    [0, -2, -2], // TX_32X64
    [2, -4, -2], // TX_64X32
    [2, -1, 0],  // TX_4X16
    [2, -1, 0],  // TX_16X4
    [2, -2, 0],  // TX_8X32
    [2, -2, 0],  // TX_32X8
    [0, -2, 0],  // TX_16X64
    [2, -4, 0],  // TX_64X16
];

/// C `fwd_cos_bit_col[MAX_TXWH_IDX][MAX_TXWH_IDX]` (transforms.c:17).
pub(super) const FWD_COS_BIT_COL: [[i8; 5]; 5] = [
    [13, 13, 13, 0, 0],
    [13, 13, 13, 12, 0],
    [13, 13, 13, 12, 13],
    [0, 13, 13, 12, 13],
    [0, 0, 13, 12, 13],
];

/// C `fwd_cos_bit_row[MAX_TXWH_IDX][MAX_TXWH_IDX]` (transforms.c:19).
pub(super) const FWD_COS_BIT_ROW: [[i8; 5]; 5] = [
    [13, 13, 12, 0, 0],
    [13, 13, 13, 12, 0],
    [13, 13, 12, 13, 12],
    [0, 12, 13, 12, 11],
    [0, 0, 12, 11, 10],
];

/// C `vtx_tab[TX_TYPES]` (inv_transforms.h:45) — 0=DCT, 1=ADST, 2=FLIPADST,
/// 3=IDTX.
pub(super) const VTX_TAB: [usize; 16] = [0, 1, 0, 1, 2, 0, 2, 1, 2, 3, 0, 3, 1, 3, 2, 3];
/// C `htx_tab[TX_TYPES]` (inv_transforms.h:63).
pub(super) const HTX_TAB: [usize; 16] = [0, 0, 1, 1, 0, 2, 2, 2, 1, 3, 3, 0, 3, 1, 3, 2];

/// (width, height) for a `TxSize` — C `tx_size_wide` / `tx_size_high`.
pub const fn tx_size_dims(tx_size: TxSize) -> (usize, usize) {
    match tx_size {
        TxSize::Tx4x4 => (4, 4),
        TxSize::Tx8x8 => (8, 8),
        TxSize::Tx16x16 => (16, 16),
        TxSize::Tx32x32 => (32, 32),
        TxSize::Tx64x64 => (64, 64),
        TxSize::Tx4x8 => (4, 8),
        TxSize::Tx8x4 => (8, 4),
        TxSize::Tx8x16 => (8, 16),
        TxSize::Tx16x8 => (16, 8),
        TxSize::Tx16x32 => (16, 32),
        TxSize::Tx32x16 => (32, 16),
        TxSize::Tx32x64 => (32, 64),
        TxSize::Tx64x32 => (64, 32),
        TxSize::Tx4x16 => (4, 16),
        TxSize::Tx16x4 => (16, 4),
        TxSize::Tx8x32 => (8, 32),
        TxSize::Tx32x8 => (32, 8),
        TxSize::Tx16x64 => (16, 64),
        TxSize::Tx64x16 => (64, 16),
    }
}

/// C `get_flip_cfg` (inv_transforms.h:139) → `(ud_flip, lr_flip)`.
pub const fn get_flip_cfg(tx_type: TxType) -> (bool, bool) {
    match tx_type {
        TxType::FlipAdstDct | TxType::FlipAdstAdst | TxType::VFlipAdst => (true, false),
        TxType::DctFlipAdst | TxType::AdstFlipAdst | TxType::HFlipAdst => (false, true),
        TxType::FlipAdstFlipAdst => (true, true),
        _ => (false, false),
    }
}

/// C `Txfm2dFlipCfg` (inv_transforms.h:103).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Txfm2dFlipCfg {
    pub tx_size: TxSize,
    pub ud_flip: bool,
    pub lr_flip: bool,
    pub shift: [i8; 3],
    pub cos_bit_col: i8,
    pub cos_bit_row: i8,
    pub stage_range_col: [i8; MAX_TXFM_STAGE_NUM],
    pub stage_range_row: [i8; MAX_TXFM_STAGE_NUM],
    pub txfm_type_col: TxfmType,
    pub txfm_type_row: TxfmType,
    pub stage_num_col: i32,
    pub stage_num_row: i32,
}

/// Port of C `set_fwd_txfm_non_scale_range` (transforms.c:3051).
///
/// Note the index that a careless read gets wrong: the ROW loop's first term
/// is `range_mult2_**col**[cfg->stage_num_col - 1]`, the COLUMN table's last
/// live entry — not the row table's.
pub(super) fn set_fwd_txfm_non_scale_range(cfg: &mut Txfm2dFlipCfg) {
    cfg.stage_range_col = [0; MAX_TXFM_STAGE_NUM];
    cfg.stage_range_row = [0; MAX_TXFM_STAGE_NUM];
    if cfg.txfm_type_col == TxfmType::Invalid {
        return;
    }
    let range_mult2_col = &FWD_TXFM_RANGE_MULT2_LIST[cfg.txfm_type_col as usize];
    let stage_num_col = (cfg.stage_num_col as usize).min(MAX_TXFM_STAGE_NUM);
    for i in 0..stage_num_col {
        cfg.stage_range_col[i] = (range_mult2_col[i] + 1) >> 1;
    }
    if cfg.txfm_type_row != TxfmType::Invalid {
        let range_mult2_row = &FWD_TXFM_RANGE_MULT2_LIST[cfg.txfm_type_row as usize];
        let stage_num_row = (cfg.stage_num_row as usize).min(MAX_TXFM_STAGE_NUM);
        for i in 0..stage_num_row {
            cfg.stage_range_row[i] =
                (range_mult2_col[cfg.stage_num_col as usize - 1] + range_mult2_row[i] + 1) >> 1;
        }
    }
}

/// Port of C `svt_aom_transform_config` (transforms.c:3074).
pub fn transform_config(tx_type: TxType, tx_size: TxSize) -> Txfm2dFlipCfg {
    let (ud_flip, lr_flip) = get_flip_cfg(tx_type);
    let (w, h) = tx_size_dims(tx_size);
    let txw_idx = w.trailing_zeros() as usize - 2;
    let txh_idx = h.trailing_zeros() as usize - 2;
    let tx_type_1d_col = VTX_TAB[tx_type as usize];
    let tx_type_1d_row = HTX_TAB[tx_type as usize];
    let txfm_type_col = AV1_TXFM_TYPE_LS[txh_idx][tx_type_1d_col];
    let txfm_type_row = AV1_TXFM_TYPE_LS[txw_idx][tx_type_1d_row];
    let stage_num_of = |t: TxfmType| -> i32 {
        if t == TxfmType::Invalid {
            // C indexes `av1_txfm_stage_num_list[TXFM_TYPE_INVALID]` out of
            // bounds here. Nothing reads the value in that case (the row loop
            // in set_fwd_txfm_non_scale_range is skipped and the kernel
            // lookup asserts), so this returns 0 rather than reproducing an
            // out-of-bounds read.
            0
        } else {
            AV1_TXFM_STAGE_NUM_LIST[t as usize] as i32
        }
    };
    let mut cfg = Txfm2dFlipCfg {
        tx_size,
        ud_flip,
        lr_flip,
        shift: FWD_TXFM_SHIFT_LS[tx_size as usize],
        cos_bit_col: FWD_COS_BIT_COL[txw_idx][txh_idx],
        cos_bit_row: FWD_COS_BIT_ROW[txw_idx][txh_idx],
        stage_range_col: [0; MAX_TXFM_STAGE_NUM],
        stage_range_row: [0; MAX_TXFM_STAGE_NUM],
        txfm_type_col,
        txfm_type_row,
        stage_num_col: stage_num_of(txfm_type_col),
        stage_num_row: stage_num_of(txfm_type_row),
    };
    set_fwd_txfm_non_scale_range(&mut cfg);
    cfg
}

/// Port of C `svt_av1_gen_fwd_stage_range` (transforms.c:733).
pub fn gen_fwd_stage_range(
    cfg: &Txfm2dFlipCfg,
    bd: i32,
) -> ([i8; MAX_TXFM_STAGE_NUM], [i8; MAX_TXFM_STAGE_NUM]) {
    let mut col = [0i8; MAX_TXFM_STAGE_NUM];
    let mut row = [0i8; MAX_TXFM_STAGE_NUM];
    let shift = cfg.shift;
    for i in 0..(cfg.stage_num_col as usize).min(MAX_TXFM_STAGE_NUM) {
        col[i] = (cfg.stage_range_col[i] as i32 + shift[0] as i32 + bd + 1) as i8;
    }
    for i in 0..(cfg.stage_num_row as usize).min(MAX_TXFM_STAGE_NUM) {
        row[i] = (cfg.stage_range_row[i] as i32 + shift[0] as i32 + shift[1] as i32 + bd + 1) as i8;
    }
    (col, row)
}

// =============================================================================
// 2-D composition — ports of `av1_tranform_two_d_core_N2_c` (transforms.c:6135)
// and `av1_tranform_two_d_core_N4_c` (:7732), plus their entry points.
// =============================================================================

/// 1-D kernel signature (C `TxfmFunc` minus the assert-only `stage_range`).
pub(super) type Kernel1D = fn(&[i32], &mut [i32], i8);

/// Port of C `fwd_txfm_type_to_func_N2` (transforms.c:6099).
///
/// `TXFM_TYPE_ADST32` maps to the FULL `av1_fadst32_new` in C — there is no
/// `_N2` variant of it — so this returns `None` for it and the caller must
/// fall back to the unpruned kernel. (`av1_fadst32_new` is unreachable in
/// practice: `av1_txfm_type_ls` only yields ADST32 for a 32-point ADST, and
/// `get_fwd_txfm_func` in `fwd_txfm.rs` has the same hole.)
pub fn fwd_txfm_type_to_func_n2(t: TxfmType) -> Option<Kernel1D> {
    Some(match t {
        TxfmType::Dct4 => fdct4_n2,
        TxfmType::Dct8 => fdct8_n2,
        TxfmType::Dct16 => fdct16_n2,
        TxfmType::Dct32 => fdct32_n2,
        TxfmType::Dct64 => fdct64_n2,
        TxfmType::Adst4 => fadst4_n2,
        TxfmType::Adst8 => fadst8_n2,
        TxfmType::Adst16 => fadst16_n2,
        TxfmType::Identity4 => fidentity4_n2,
        TxfmType::Identity8 => fidentity8_n2,
        TxfmType::Identity16 => fidentity16_n2,
        TxfmType::Identity32 => fidentity32_n2,
        TxfmType::Identity64 => fidentity64_n2,
        // TXFM_TYPE_ADST32 -> av1_fadst32_new (unpruned) / TXFM_TYPE_INVALID
        TxfmType::Adst32 | TxfmType::Invalid => return None,
    })
}

/// Port of C `fwd_txfm_type_to_func_N4` (transforms.c:7696). Same ADST32
/// hole as [`fwd_txfm_type_to_func_n2`].
pub fn fwd_txfm_type_to_func_n4(t: TxfmType) -> Option<Kernel1D> {
    Some(match t {
        TxfmType::Dct4 => fdct4_n4,
        TxfmType::Dct8 => fdct8_n4,
        TxfmType::Dct16 => fdct16_n4,
        TxfmType::Dct32 => fdct32_n4,
        TxfmType::Dct64 => fdct64_n4,
        TxfmType::Adst4 => fadst4_n4,
        TxfmType::Adst8 => fadst8_n4,
        TxfmType::Adst16 => fadst16_n4,
        TxfmType::Identity4 => fidentity4_n4,
        TxfmType::Identity8 => fidentity8_n4,
        TxfmType::Identity16 => fidentity16_n4,
        TxfmType::Identity32 => fidentity32_n4,
        TxfmType::Identity64 => fidentity64_n4,
        TxfmType::Adst32 | TxfmType::Invalid => return None,
    })
}
