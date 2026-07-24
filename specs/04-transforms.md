# Transforms & Quantization

## Overview

SVT-AV1 implements the complete AV1 transform and quantization pipeline: forward transforms convert spatial-domain residual blocks into frequency-domain coefficients, quantization reduces their precision for entropy coding, and inverse transforms reconstruct the spatial-domain signal during decoding.

The transform system supports 4 basis transform types (DCT, ADST, flip-ADST, identity) combined into 16 2D transform types across 19 transform sizes (4x4 through 64x64, including rectangular). The quantization system uses AV1's qindex-based quantizer lookup with optional quantization matrices, dead-zone quantization, and rate-distortion optimized quantization (RDOQ).

The forward path additionally supports "partial frequency" (PF) coefficient shapes (DEFAULT, N2, N4, ONLY_DC) that compute only a subset of transform coefficients, trading quality for speed at higher presets.

## Source Files

| File | Lines | Purpose |
|------|-------|---------|
| `Source/Lib/Codec/transforms.c` | 7958 | Forward transforms: 1D DCT/ADST/identity kernels, 2D transform core, WHT, coefficient shape variants (N2, N4), transform dispatch |
| `Source/Lib/Codec/transforms.h` | 117 | Forward transform declarations, `QuantParam`, shift tables, `q_func` constant |
| `Source/Lib/Codec/inv_transforms.c` | 3517 | Inverse transforms: 1D IDCT/IADST/iidentity kernels, 2D inverse core, inverse WHT, quantization lookup tables (DC/AC for 8/10/12-bit), `svt_aom_dc_quant_qtx`/`svt_aom_ac_quant_qtx` |
| `Source/Lib/Codec/inv_transforms.h` | 372 | Inverse transform declarations, `Txfm2dFlipCfg`, `TxfmType` enum, cospi/sinpi arrays, `half_btf`, `round_shift`, scan tables |
| `Source/Lib/Codec/coefficients.c` | 1593 | Coefficient scan orders (default diagonal, row, column) for all 19 TX sizes, `eb_av1_scan_orders` table, nz map context offsets |
| `Source/Lib/Codec/coefficients.h` | 214 | Coefficient context computation: `get_nz_mag`, `get_lower_levels_ctx`, `get_br_ctx`, `ScanOrder` access, `tx_size_2d` table |
| `Source/Lib/Codec/fft.c` | 237 | Float-point FFT/IFFT: 2D gen functions, 1D butterfly macros for sizes 2-32, used for noise/grain analysis |
| `Source/Lib/Codec/fft_common.h` | ~110 | FFT function pointer types, macro generators (GEN_FFT_2 through GEN_FFT_32), `svt_aom_fft_2d_gen`/`svt_aom_ifft_2d_gen` |
| `Source/Lib/Codec/lambda_rate_tables.h` | 79 | Lambda tables for mode decision (SAD-domain) at 8-bit, 10-bit, and 12-bit, indexed by qindex (0-255) |
| `Source/Lib/Codec/q_matrices.h` | ~933KB | Quantization weight matrices (`wt_matrix_ref`), `NUM_QM_LEVELS` levels, per-plane per-TX-size, compile-gated by `CONFIG_ENABLE_QUANT_MATRIX` |
| `Source/Lib/Codec/full_loop.c` | (partial) | Contains `svt_aom_quantize_inv_quantize`, `svt_aom_quantize_inv_quantize_light`, RDOQ (`svt_av1_optimize_b`) |

## Test Coverage

| Test File | What It Tests |
|-----------|--------------|
| `test/ForwardtransformTests.cc` | Forward transform correctness for all sizes |
| `test/FwdTxfm1dTest.cc` | 1D forward DCT/ADST/identity kernels |
| `test/FwdTxfm2dTest.cc` | 2D forward transforms, reference comparison |
| `test/FwdTxfm2dAsmTest.cc` | SIMD forward transform correctness vs C reference |
| `test/FwdTxfm2dApproxTest.cc` | Approximate forward transform variants |
| `test/InvTxfm1dTest.cc` | 1D inverse DCT/IADST/iidentity kernels |
| `test/InvTxfm2dAsmTest.cc` | SIMD inverse transform correctness vs C reference |
| `test/EncodeTxbAsmTest.cc` | Transform block encoding (txb_init_levels, compute_cul_level) |
| `test/ResidualTest.cc` | Residual computation |
| `test/hadamard_test.cc` | Hadamard transform |
| `test/QuantAsmTest.cc` | Quantization SIMD vs C |
| `test/quantize_func_test.cc` | Quantization function correctness |
| `test/FFTTest.cc` | FFT/IFFT float operations |
| `test/TxfmCommon.h` | Shared test utilities for transform tests |

## Data Structures

### TxfmType (1D transform type enum)

```
TXFM_TYPE_DCT4, TXFM_TYPE_DCT8, TXFM_TYPE_DCT16, TXFM_TYPE_DCT32, TXFM_TYPE_DCT64
TXFM_TYPE_ADST4, TXFM_TYPE_ADST8, TXFM_TYPE_ADST16, TXFM_TYPE_ADST32
TXFM_TYPE_IDENTITY4, TXFM_TYPE_IDENTITY8, TXFM_TYPE_IDENTITY16, TXFM_TYPE_IDENTITY32, TXFM_TYPE_IDENTITY64
```

### TxType (2D transform type enum, 16 types)

Each 2D type decomposes into a vertical (column) 1D type and a horizontal (row) 1D type:

| TxType | Vertical (col) | Horizontal (row) | ud_flip | lr_flip |
|--------|----------------|-------------------|---------|---------|
| `DCT_DCT` (0) | DCT | DCT | 0 | 0 |
| `ADST_DCT` (1) | ADST | DCT | 0 | 0 |
| `DCT_ADST` (2) | DCT | ADST | 0 | 0 |
| `ADST_ADST` (3) | ADST | ADST | 0 | 0 |
| `FLIPADST_DCT` (4) | FLIPADST | DCT | 1 | 0 |
| `DCT_FLIPADST` (5) | DCT | FLIPADST | 0 | 1 |
| `FLIPADST_FLIPADST` (6) | FLIPADST | FLIPADST | 1 | 1 |
| `ADST_FLIPADST` (7) | ADST | FLIPADST | 0 | 1 |
| `FLIPADST_ADST` (8) | FLIPADST | ADST | 1 | 0 |
| `IDTX` (9) | IDENTITY | IDENTITY | 0 | 0 |
| `V_DCT` (10) | DCT | IDENTITY | 0 | 0 |
| `H_DCT` (11) | IDENTITY | DCT | 0 | 0 |
| `V_ADST` (12) | ADST | IDENTITY | 0 | 0 |
| `H_ADST` (13) | IDENTITY | ADST | 0 | 0 |
| `V_FLIPADST` (14) | FLIPADST | IDENTITY | 1 | 0 |
| `H_FLIPADST` (15) | IDENTITY | FLIPADST | 0 | 1 |

The column/row decomposition is defined by `vtx_tab[TX_TYPES]` (vertical) and `htx_tab[TX_TYPES]` (horizontal). FLIPADST is implemented as ADST with input reversal (ud_flip for vertical, lr_flip for horizontal).

### TxType1D (1D basis types)

```
DCT_1D (0), ADST_1D (1), FLIPADST_1D (2), IDTX_1D (3)
```

Note: FLIPADST_1D maps to the same 1D kernel as ADST_1D in `av1_txfm_type_ls`; the flip is applied by reversing input order.

### TxSize (19 sizes)

| Enum | Size | Square | Rectangular |
|------|------|--------|-------------|
| `TX_4X4` (0) | 4x4 | Yes | - |
| `TX_8X8` (1) | 8x8 | Yes | - |
| `TX_16X16` (2) | 16x16 | Yes | - |
| `TX_32X32` (3) | 32x32 | Yes | - |
| `TX_64X64` (4) | 64x64 | Yes | - |
| `TX_4X8` (5) | 4x8 | - | 1:2 |
| `TX_8X4` (6) | 8x4 | - | 2:1 |
| `TX_8X16` (7) | 8x16 | - | 1:2 |
| `TX_16X8` (8) | 16x8 | - | 2:1 |
| `TX_16X32` (9) | 16x32 | - | 1:2 |
| `TX_32X16` (10) | 32x16 | - | 2:1 |
| `TX_32X64` (11) | 32x64 | - | 1:2 |
| `TX_64X32` (12) | 64x32 | - | 2:1 |
| `TX_4X16` (13) | 4x16 | - | 1:4 |
| `TX_16X4` (14) | 16x4 | - | 4:1 |
| `TX_8X32` (15) | 8x32 | - | 1:4 |
| `TX_32X8` (16) | 32x8 | - | 4:1 |
| `TX_16X64` (17) | 16x64 | - | 1:4 |
| `TX_64X16` (18) | 64x16 | - | 4:1 |

### TxClass

```
TX_CLASS_2D (0)    - both dimensions use frequency transforms (DCT/ADST)
TX_CLASS_HORIZ (1) - horizontal frequency, vertical identity (H_DCT, H_ADST, H_FLIPADST)
TX_CLASS_VERT (2)  - vertical frequency, horizontal identity (V_DCT, V_ADST, V_FLIPADST)
```

### TxCoeffShape (partial frequency shapes)

```
DEFAULT_SHAPE (0) - compute all coefficients
N2_SHAPE (1)      - compute only the top-left N/2 x N/2 coefficients
N4_SHAPE (2)      - compute only the top-left N/4 x N/4 coefficients
ONLY_DC_SHAPE (3) - compute only the DC coefficient
```

### Txfm2dFlipCfg

The configuration struct for a 2D transform:

```c
struct Txfm2dFlipCfg {
    TxSize        tx_size;
    int32_t       ud_flip;           // flip upside down (reverse column input)
    int32_t       lr_flip;           // flip left to right (reverse column output placement)
    const int8_t* shift;             // [3] for fwd: {input_shift, col_shift, row_shift}
                                     // [2] for inv: {row_shift, col_shift}
    int8_t        cos_bit_col;       // cosine precision for column transform
    int8_t        cos_bit_row;       // cosine precision for row transform
    int8_t        stage_range_col[MAX_TXFM_STAGE_NUM]; // bit range at each stage
    int8_t        stage_range_row[MAX_TXFM_STAGE_NUM];
    TxfmType      txfm_type_col;     // 1D transform type for columns
    TxfmType      txfm_type_row;     // 1D transform type for rows
    int32_t       stage_num_col;     // number of stages in column transform
    int32_t       stage_num_row;     // number of stages in row transform
};
```

### QuantParam

```c
struct QuantParam {
    int32_t      log_scale;   // tx scale: 0, 1, or 2 (based on tx_size_2d > 256, > 1024)
    TxSize       tx_size;
    const QmVal* qmatrix;     // quantization matrix (NULL when flat)
    const QmVal* iqmatrix;    // inverse quantization matrix (NULL when flat)
};
```

### ScanOrder

```c
struct ScanOrder {
    const int16_t* scan;   // scan order: maps position index -> raster index
    const int16_t* iscan;  // inverse scan: maps raster index -> position index
};
```

## Algorithms

### 1. Forward Transform Configuration

`svt_aom_transform_config(tx_type, tx_size, cfg)` builds a `Txfm2dFlipCfg`:

1. Set `tx_size` in cfg.
2. Set flip flags from `get_flip_cfg(tx_type)`.
3. Decompose `tx_type` into 1D column type via `vtx_tab[tx_type]` and 1D row type via `htx_tab[tx_type]`.
4. Compute `txw_idx = log2(width) - 2` and `txh_idx = log2(height) - 2` (both 0-4).
5. Look up shift from `fwd_txfm_shift_ls[tx_size]` (array of 3 values: input shift, inter-stage shift, output shift).
6. Look up cos_bit from `fwd_cos_bit_col[txw_idx][txh_idx]` and `fwd_cos_bit_row[txw_idx][txh_idx]`.
7. Look up 1D transform type from `av1_txfm_type_ls[txh_idx][col_type_1d]` for columns and `av1_txfm_type_ls[txw_idx][row_type_1d]` for rows.
8. Get stage counts from `av1_txfm_stage_num_list[txfm_type]`.
9. Compute stage ranges from `fwd_txfm_range_mult2_list`.

### 2. Forward 2D Transform Core

`av1_tranform_two_d_core_c(input, input_stride, output, cfg, buf, bit_depth)`:

**Phase 1: Columns** (for each column c = 0..width-1):
1. Extract column from input (applying ud_flip if set: reverse row order).
2. Apply input shift: `round_shift_array(temp_in, height, -shift[0])`.
3. Apply 1D column transform: `txfm_func_col(temp_in, temp_out, cos_bit_col, stage_range_col)`.
4. Apply inter-stage shift: `round_shift_array(temp_out, height, -shift[1])`.
5. Store to intermediate buffer (applying lr_flip if set: reverse column placement).

**Phase 2: Rows** (for each row r = 0..height-1):
1. Apply 1D row transform to intermediate buffer row.
2. Apply output shift: `round_shift_array(output_row, width, -shift[2])`.
3. If rectangular with 2:1 or 1:2 ratio (`|rect_type| == 1`): multiply all coefficients by `sqrt(2)` via `round_shift(coeff * 5793, 12)`.

### 3. Forward Shift Tables

Each TX size has a 3-element shift array `{shift[0], shift[1], shift[2]}`:

| TX Size | shift[0] | shift[1] | shift[2] | Notes |
|---------|----------|----------|----------|-------|
| 4x4 | 2 | 0 | 0 | |
| 8x8 | 2 | -1 | 0 | |
| 16x16 | 2 | -2 | 0 | |
| 32x32 | 2 | -4 | 0 | |
| 64x64 | 0 | -2 | -2 | Special: no input shift |
| 4x8, 8x4 | 2 | -1 | 0 | |
| 8x16, 16x8 | 2 | -2 | 0 | |
| 16x32, 32x16 | 2 | -4 | 0 | |
| 32x64 | 0 | -2 | -2 | |
| 64x32 | 2 | -4 | -2 | |
| 4x16, 16x4 | 2 | -1 | 0 | |
| 8x32, 32x8 | 2 | -2 | 0 | |
| 16x64 | 0 | -2 | 0 | |
| 64x16 | 2 | -4 | 0 | |

Negative shifts mean left-shift (multiply by power of 2). The `round_shift_array` function handles both directions.

### 4. Forward 1D DCT

All DCT kernels follow the standard recursive butterfly structure. The fundamental building block is `half_btf`:

```
half_btf(w0, in0, w1, in1, bit) = round_shift((int64_t)(w0 * in0) + (int64_t)(w1 * in1), bit)
```

Where `round_shift(value, bit) = (value + (1 << (bit-1))) >> bit`.

Cosine values are stored as fixed-point integers: `cospi[k] = round(cos(k * pi/64) * (1 << cos_bit))`. The `cos_bit` for forward transforms is typically 13 (columns) or 10-13 (rows, depending on size).

**DCT-4** (4 stages, 3 with computation):
- Stage 1: Butterfly additions (input[i] +/- input[3-i])
- Stage 2: cospi[32] rotations (DC/Nyquist) and cospi[48]/cospi[16] rotation
- Stage 3: Bit-reversal reorder

**DCT-8** (6 stages): Recursive split into two DCT-4 subproblems with additional cospi rotations.

**DCT-16** (8 stages): Recursive split into DCT-8 + 8-point subproblem.

**DCT-32** (10 stages): Recursive split into DCT-16 + 16-point subproblem.

**DCT-64** (12 stages): Recursive split into DCT-32 + 32-point subproblem.

### 5. Forward 1D ADST

ADST (Asymmetric Discrete Sine Transform) uses sine-based twiddle factors via `sinpi_arr`.

**ADST-4** (7 stages): Uses `sinpi[1..4]` coefficients. The algorithm:
1. Compute weighted sums: `s0 = sinpi[1]*x0`, `s1 = sinpi[4]*x0`, `s2 = sinpi[2]*x1`, etc.
2. Combine: `x0 = s0 + s2 + s5`, `x1 = sinpi[3]*(x0+x1-x3)`, `x2 = s1 - s3 + s6`, `x3 = s4`
3. Final butterfly: `output = {x0+x3, x1, x2-x3, x2-x0+x3}`
4. Round-shift all outputs by cos_bit.

**ADST-8, ADST-16, ADST-32**: Use cospi-based butterfly structures (not sinpi). They follow a different stage pattern than DCT but still use `half_btf` with various cospi rotation angles. Stage counts: 8, 10, 12 respectively.

### 6. Forward 1D Identity Transform

The identity transform preserves spatial-domain values with a size-dependent scale factor:

| Size | Scale | Implementation |
|------|-------|----------------|
| 4 | sqrt(2) | `round_shift(input[i] * 5793, 12)` |
| 8 | 2 | `input[i] * 2` |
| 16 | 2*sqrt(2) | `round_shift(input[i] * 2 * 5793, 12)` |
| 32 | 4 | `input[i] * 4` |
| 64 | 4*sqrt(2) | `round_shift(input[i] * 4 * 5793, 12)` |

Constants: `new_sqrt2 = 5793` (= round(sqrt(2) * 2^12)), `new_sqrt2_bits = 12`.

### 7. Walsh-Hadamard Transform (WHT)

Used in lossless mode (qindex == 0). Only 4x4 is supported.

`svt_av1_fwht4x4_c(input, output, stride)`:
- Two passes (columns then rows), each performing the same butterfly:
  ```
  a += b; d -= c; e = (a - d) >> 1; b = e - b; c = e - c; a -= c; d += b;
  output = {a, c, d, b}
  ```
- The row pass multiplies results by `UNIT_QUANT_FACTOR` (4).

For the inverse, `svt_av1_highbd_iwht4x4_16_add_c` reverses this process.

### 8. Inverse Transform Configuration

`svt_av1_get_inv_txfm_cfg(tx_type, tx_size, cfg)`:

Similar to forward but uses:
- `svt_aom_inv_txfm_shift_ls[tx_size]` (2-element arrays, not 3)
- `inv_cos_bit_col/row` (all entries are `INV_COS_BIT = 12`)
- Special handling for IADST-4: copies `iadst4_range` into stage_range

### 9. Inverse 2D Transform Core

`inv_txfm2d_add_c(input, output_r, stride_r, output_w, stride_w, cfg, txfm_buf, tx_size, bd)`:

The inverse is the transpose of the forward: rows first, then columns, with addition to the prediction.

**Phase 1: Rows** (for each row r):
1. If rectangular (2:1 ratio): multiply input by `new_inv_sqrt2` (= 2896 = round(2^12 / sqrt(2))).
2. Clamp input to `bd + 8` bits.
3. Apply 1D row transform.
4. Apply row shift: `round_shift_array(buf_ptr, width, -shift[0])`.

**Phase 2: Columns** (for each column c):
1. Extract column from intermediate buffer (applying lr_flip if set).
2. Clamp to `max(bd + 6, 16)` bits.
3. Apply 1D column transform.
4. Apply column shift: `round_shift_array(temp_out, height, -shift[1])`.
5. Add to prediction and clip: `output_w[r][c] = clip_pixel(output_r[r][c] + temp_out[r], bd)`.
   - ud_flip reverses the row order of temp_out during addition.

### 10. Inverse Shift Tables

| TX Size | shift[0] (row) | shift[1] (col) |
|---------|----------------|-----------------|
| 4x4 | 0 | -4 |
| 8x8 | -1 | -4 |
| 16x16 | -2 | -4 |
| 32x32 | -2 | -4 |
| 64x64 | -2 | -4 |
| 4x8, 8x4 | 0 | -4 |
| 8x16, 16x8 | -1 | -4 |
| 16x32, 32x16 | -1 | -4 |
| 32x64, 64x32 | -1 | -4 |
| 4x16, 16x4 | -1 | -4 |
| 8x32, 32x8 | -2 | -4 |
| 16x64, 64x16 | -2 | -4 |

### 11. Inverse 1D Transforms

**IDCT-4** (3 stages):
1. Input reorder (bit-reversal): `{input[0], input[2], input[1], input[3]}`
2. cospi[32] and cospi[48]/cospi[16] rotations via `half_btf`
3. Butterfly additions with clamping: `clamp_value(bf0[i] +/- bf0[j], stage_range)`

**IDCT-8, 16, 32, 64**: Follow the same recursive butterfly pattern as forward DCT but in reverse order.

**IADST-4**: Uses sinpi values. The inverse is mathematically the transpose of the forward ADST.

**IADST-8, 16, 32**: Use cospi butterflies in reverse stage order compared to forward ADST.

**Iidentity-N**: Same scale factors as forward identity:
- 4: `round_shift(new_sqrt2 * input[i], 12)`
- 8: `input[i] * 2`
- 16: `round_shift(new_sqrt2 * 2 * input[i], 12)`
- 32: `input[i] * 4`
- 64: `round_shift(new_sqrt2 * 4 * input[i], 12)`

### 12. 64-Point Transform Coefficient Zeroing

For transforms involving 64-point dimensions, AV1 zeros out the upper-right and bottom halves of the coefficient matrix:

- **64x64**: Only the top-left 32x32 of the 64x64 output is retained. `svt_handle_transform64x64` computes "three_quad_energy" from the discarded coefficients, then repacks the 32x32 into contiguous memory. Max EOB is 1024 (not 4096).
- **64x32, 32x64**: Max EOB is 1024.
- **64x16, 16x64**: Max EOB is 512.

The `av1_get_max_eob` function implements this logic.

### 13. Cosine/Sine Lookup Tables

**Cosine table**: `svt_aom_eb_av1_cospi_arr_data[7][64]` stores `cospi[k] = round(cos(k*pi/64) * (1 << cos_bit))` for `cos_bit` from 10 to 16 (index = cos_bit - 10).

**Sine table**: `svt_aom_eb_av1_sinpi_arr_data[7][5]` stores `sinpi[k]` for k=1..4, used only by ADST-4/IADST-4.

Accessed via `cospi_arr(n)` and `sinpi_arr(n)` where n is the cos_bit value.

### 14. Quantization

#### QIndex to Quantizer Step

AV1 uses qindex (0-255) mapped to quantizer step sizes via lookup tables. Separate tables exist for DC and AC coefficients at each bit depth:

| Bit Depth | DC Lookup | AC Lookup |
|-----------|-----------|-----------|
| 8-bit | `dc_qlookup_QTX[256]` | `ac_qlookup_QTX[256]` |
| 10-bit | `dc_qlookup_10_QTX[256]` | `ac_qlookup_10_QTX[256]` |
| 12-bit | `dc_qlookup_12_QTX[256]` | `ac_qlookup_12_QTX[256]` |

The lookup is: `dc_quant = dc_qlookup[clamp(qindex + delta, 0, 255)]` and similarly for AC.

AV1 transform coefficients are always scaled up by a factor of 8 (3 bits), so these are "Q3" quantizers.

#### Quantization Parameters Setup

For each transform block, the quantization function selects parameters per-plane and per-qindex:
- `quant_qtx`: quantizer multiplier
- `quant_fp_qtx`: full-precision quantizer (for RDOQ path)
- `round_fp_qtx`: rounding offset (for RDOQ path)
- `quant_shift_qtx`: shift amount
- `zbin_qtx`: dead zone boundary
- `round_qtx`: rounding value
- `dequant_qtx`: dequantizer (2-element: `[0]` = DC, `[1]` = AC)

These are pre-computed for all 256 qindex values during encoder init, stored in `EncodeContext.quants_8bit` / `quants_bd` and `deq_8bit` / `deq_bd`.

#### TX Scale

Large transforms scale quantization:
```
log_scale = (tx_size_2d[tx_size] > 256) + (tx_size_2d[tx_size] > 1024)
```
- 0 for sizes up to 256 pels (16x16 and smaller)
- 1 for 512-1024 pels (32x16, 16x32, 32x32)
- 2 for >1024 pels (32x64, 64x32, 64x64)

#### Dead-Zone Quantization (`svt_aom_quantize_b`)

The standard AV1 dead-zone quantizer:
```
for each coefficient in scan order:
    abs_coeff = |coeff[pos]|
    if abs_coeff >= zbin[is_dc]:       // dead zone test
        q = (abs_coeff + round[is_dc]) * quant[is_dc]  // multiply
        q = q >> (16 + log_scale)                        // shift
        if q > 0:
            qcoeff[pos] = sign(coeff[pos]) * q
            dqcoeff[pos] = sign * q * dequant[is_dc] >> log_scale
            eob = max(eob, iscan[pos])
```

When quantization matrices are active (`qm_ptr != NULL`), the dead zone and quantizer are adjusted per-coefficient:
```
zbin_adjusted = (zbin[is_dc] * qm[pos] + 32) >> AOM_QM_BITS
quant_adjusted = quant_fp[is_dc]
round_adjusted = (round_fp[is_dc] * iqm[pos] + 32) >> AOM_QM_BITS
```

Where `AOM_QM_BITS = 5` and the matrix values are 5-bit fixed point.

#### Full-Precision Quantization (`svt_av1_quantize_fp`)

Used in the RDOQ path. Skips the dead-zone test (no zbin comparison) and uses `quant_fp` / `round_fp` instead:
```
q = (abs_coeff * quant_fp[is_dc] + round_fp[is_dc]) >> (16 + log_scale)
```

This gives better RD performance at the cost of potentially keeping more non-zero coefficients.

#### Quantization Matrix (`q_matrices.h`)

When enabled (`CONFIG_ENABLE_QUANT_MATRIX`), the quantization matrix `wt_matrix_ref[NUM_QM_LEVELS][2][QM_TOTAL_SIZE]` provides per-coefficient weights. Indexed as:
- Level 0-15 (NUM_QM_LEVELS): quality levels from most aggressive to flattest
- Plane 0-1: luma and chroma
- Linearized for all TX sizes within `QM_TOTAL_SIZE = 3344` entries

Level `NUM_QM_LEVELS - 1` is the flat matrix (all 32), meaning no weighting. Lower levels apply stronger frequency weighting (lower values at high frequencies = more quantization).

Only used when `frm_hdr.quantization_params.using_qmatrix` is true and the transform is 2D (not identity).

#### Dequantization

Dequantization is embedded in the quantize functions:
```
dqcoeff[pos] = (qcoeff[pos] * dequant[is_dc]) >> log_scale
```

For quantization matrix:
```
dqcoeff[pos] = (qcoeff[pos] * dequant[is_dc] * iqm[pos] + (1 << (AOM_QM_BITS - 1))) >> AOM_QM_BITS
dqcoeff[pos] >>= log_scale
```

#### Invert Quant Helper

`svt_aom_invert_quant(quant, shift, d)`:
- Computes the fixed-point reciprocal of dequant value `d`.
- `l = floor(log2(d))`
- `m = 1 + (1 << (16 + l)) / d`
- `*quant = m - (1 << 16)` (stored as int16_t)
- `*shift = 1 + l + 16` (used with right-shift in the quantizer)

#### QZBin Factor

`svt_aom_get_qzbin_factor(q, bit_depth)`:
- Returns 64 if q == 0 (lossless)
- Returns 84 if quant < threshold (148 / 592 / 2368 for 8/10/12-bit)
- Returns 80 otherwise

### 15. Rate-Distortion Optimized Quantization (RDOQ)

RDOQ is controlled by `ctx->rdoq_ctrls` and proceeds in several stages within `svt_aom_quantize_inv_quantize`:

**Stage 1: Gate check**
- Skip RDOQ if lossless segment, or if `rdoq_ctrls.enabled` is false.
- Skip for non-DCT_DCT types if `rdoq_ctrls.dct_dct_only` is set.
- Skip for chroma if `rdoq_ctrls.skip_uv` is set.

**Stage 2: Initial quantization**
- If RDOQ enabled and `fp_q_y`/`fp_q_uv` flags are set: use full-precision quantization (`svt_av1_quantize_fp_facade`).
- Otherwise: use dead-zone quantization (`av1_quantize_b_facade_ii`).

**Stage 3: EOB-based gating**
- Compute `eob_perc = eob * 100 / (width * height)`.
- If `eob_perc >= rdoq_ctrls.eob_th`: fall back to standard quantization (skip RDOQ).
- If `eob_perc >= rdoq_ctrls.eob_fast_th`: apply fast optimization (`svt_fast_optimize_b`) which does simple coefficient-level zeroing.

**Stage 4: Full RDOQ (`svt_av1_optimize_b`)**
- Iterates coefficients from EOB backward to 0 in scan order.
- For each coefficient, evaluates the RD cost of: keeping the quantized value, reducing it by 1, or zeroing it.
- Uses entropy cost from `LvMapCoeffCost` and `LvMapEobCost` tables.
- The RD multiplier is: `rdmult = (lambda * plane_rd_mult * rweight / 100 + 2) >> rshift`.
- Sharpness control: if enabled and delta-q indicates lower-than-average QP, `rweight` is set to 0 (maximizing coefficient preservation for sharpness).
- Updates EOB, qcoeff, and dqcoeff in-place.

**CUL Level Computation**

After quantization, `svt_av1_compute_cul_level` computes the cumulative level (sum of absolute coefficient values, capped at `COEFF_CONTEXT_MASK`) along with the DC sign context. This is used for entropy coding context.

### 16. Coefficient Scanning Orders

Three scan order patterns exist per TX size, selected by `tx_type_to_scan_index[tx_type]`:

| Scan Index | Pattern | Used By |
|------------|---------|---------|
| 0 (default) | Diagonal zigzag | DCT_DCT, ADST_DCT, DCT_ADST, ADST_ADST, FLIPADST_DCT, DCT_FLIPADST, FLIPADST_FLIPADST, ADST_FLIPADST, FLIPADST_ADST, IDTX |
| 1 (mrow) | Row-major | V_DCT, V_ADST, V_FLIPADST (horizontal identity) |
| 2 (mcol) | Column-major | H_DCT, H_ADST, H_FLIPADST (vertical identity) |

The mapping: `tx_type_to_scan_index = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 2, 1, 2}`.

For 64-point dimensions, the scan order of the next smaller power-of-2 square is reused (e.g., TX_64X64 uses TX_32X32's scan because the upper half is zeroed).

Each `ScanOrder` struct contains:
- `scan[n]`: maps scan position index to raster order index
- `iscan[n]`: maps raster order index to scan position index

### 17. Coefficient Context Computation

The `get_lower_levels_ctx` function computes the significance context for each coefficient during entropy coding:

1. Compute `nz_mag` from neighboring quantized levels (in a padded buffer with `TX_PAD_HOR` padding):
   - **TX_CLASS_2D**: neighbors at offsets {0,1}, {1,0}, {1,1}, {0,2}, {2,0} (5 neighbors, each clamped to 3)
   - **TX_CLASS_HORIZ**: neighbors at {0,1}, {1,0}, {0,2}, {0,3}, {0,4}
   - **TX_CLASS_VERT**: neighbors at {0,1}, {1,0}, {2,0}, {3,0}, {4,0}

2. Map to context via `get_nz_map_ctx_from_stats`:
   - `ctx = min((nz_mag + 1) >> 1, 4)`
   - Add offset from `eb_av1_nz_map_ctx_offset[tx_size][coeff_idx]` for TX_CLASS_2D
   - Or from `nz_map_ctx_offset_1d[col/row]` for HORIZ/VERT

### 18. Lambda Tables

`lambda_rate_tables.h` defines SAD-domain lambda tables for mode decision, indexed by qindex (0-255):

- `av1_lambda_mode_decision8_bit_sad[256]`: 8-bit
- `av1lambda_mode_decision10_bit_sad[256]`: 10-bit
- `av1lambda_mode_decision12_bit_sad[256]`: 12-bit

Values range from ~86 (qindex 0) to ~28943 (qindex 255) for 8-bit. These are used to weight distortion vs. rate in mode decision. The RDOQ lambda is derived differently, from the mode-decision lambda and `plane_rd_mult` scaling.

### 19. FFT (Floating Point)

The FFT subsystem operates on float data for noise analysis and film grain. It is separate from the integer transform pipeline.

**1D FFT sizes**: 2, 4, 8, 16, 32 points, generated via C macros (`GEN_FFT_2` through `GEN_FFT_32`).

**2D FFT** (`svt_aom_fft_2d_gen`):
1. Transform all rows (with stride).
2. Transpose result.
3. Transform all rows again (effectively columns).
4. Transpose again.
5. Unpack conjugate-symmetric results into real+imaginary pairs.

Output format: `output[2*(y*n+x)]` = real, `output[2*(y*n+x)+1]` = imaginary.

**2D IFFT** (`svt_aom_ifft_2d_gen`): Reverses the process, handling the conjugate symmetry packing.

Available sizes: 2x2, 4x4, 8x8, 16x16, 32x32.

## Key Functions

### Forward Transform Pipeline

| Function | Signature | Purpose |
|----------|-----------|---------|
| `svt_aom_estimate_transform` | `(pcs, ctx, residual, stride, coeff, coeff_stride, tx_size, three_quad_energy, bit_depth, tx_type, plane, shape)` | Top-level forward transform entry point. Dispatches by TxCoeffShape. |
| `svt_aom_transform_config` | `(tx_type, tx_size, cfg) -> void` | Build Txfm2dFlipCfg for any type/size combination |
| `av1_tranform_two_d_core_c` | `(input, stride, output, cfg, buf, bit_depth)` | Generic 2D forward transform (columns then rows) |
| `svt_av1_fwd_txfm2d_NxM` | `(input, output, stride, tx_type, bit_depth)` | Size-specific 2D forward transforms (19 sizes) |
| `svt_av1_fwd_txfm2d_NxM_N2` | Same | N2 variants: compute only top-left half |
| `svt_av1_fwd_txfm2d_NxM_N4` | Same | N4 variants: compute only top-left quarter |
| `svt_aom_fwd_txfm_type_to_func` | `(TxfmType) -> TxfmFunc` | Map 1D type enum to function pointer |
| `svt_av1_highbd_fwd_txfm` | `(src_diff, coeff, diff_stride, txfm_param)` | Dispatcher for default-shape forward transforms |
| `svt_av1_highbd_fwd_txfm_n2` | Same | Dispatcher for N2-shape forward transforms |
| `svt_av1_highbd_fwd_txfm_n4` | Same | Dispatcher for N4-shape forward transforms |
| `svt_av1_wht_fwd_txfm` | `(src_diff, bw, coeff, tx_size, pf_shape, bit_depth, is_hbd)` | Forward WHT + PF shape dispatch (used in TPL) |

### 1D Forward Kernels

| Function | Size | Stages |
|----------|------|--------|
| `svt_av1_fdct4_new` | DCT-4 | 4 |
| `svt_av1_fdct8_new` | DCT-8 | 6 |
| `svt_av1_fdct16_new` | DCT-16 | 8 |
| `svt_av1_fdct32_new` | DCT-32 | 10 |
| `svt_av1_fdct64_new` | DCT-64 | 12 |
| `svt_av1_fadst4_new` | ADST-4 | 7 (sinpi-based) |
| `svt_av1_fadst8_new` | ADST-8 | 8 |
| `svt_av1_fadst16_new` | ADST-16 | 10 |
| `av1_fadst32_new` | ADST-32 | 12 |
| `svt_av1_fidentity{4,8,16,32}_c` | Identity | 1 |
| `av1_fidentity64_c` | Identity-64 | 1 |

N2 and N4 variants exist for all kernels (e.g., `svt_av1_fdct16_new_N2`, `svt_av1_fadst4_new_N4`). These compute only the first half or quarter of outputs respectively.

### Inverse Transform Pipeline

| Function | Purpose |
|----------|---------|
| `svt_aom_inv_transform_recon` | Top-level inverse transform + reconstruction (high bit depth) |
| `svt_aom_inv_transform_recon8bit` | Top-level inverse transform + reconstruction (8-bit) |
| `svt_av1_get_inv_txfm_cfg` | Build inverse Txfm2dFlipCfg |
| `inv_txfm2d_add_c` | Generic 2D inverse transform core (rows then columns, add to prediction) |
| `svt_av1_inv_txfm2d_add_NxM_c` | Size-specific 2D inverse transforms |
| `svt_aom_inv_txfm_type_to_func` | Map 1D type enum to inverse function pointer |
| `svt_av1_highbd_iwht4x4_16_add_c` | Inverse WHT 4x4 (lossless mode) |
| `svt_av1_highbd_iwht4x4_1_add_c` | Inverse WHT 4x4 with single non-zero coeff |
| `svt_av1_gen_inv_stage_range` | Compute inverse stage ranges |

### 1D Inverse Kernels

| Function | Size |
|----------|------|
| `svt_av1_idct4_new` | IDCT-4 |
| `svt_av1_idct8_new` | IDCT-8 |
| `svt_av1_idct16_new` | IDCT-16 |
| `svt_av1_idct32_new` | IDCT-32 |
| `svt_av1_idct64_new` | IDCT-64 |
| `svt_av1_iadst4_new` | IADST-4 |
| `svt_av1_iadst8_new` | IADST-8 |
| `svt_av1_iadst16_new` | IADST-16 |
| `av1_iadst32_new` | IADST-32 |
| `svt_av1_iidentity{4,8,16,32}_c` | Iidentity |
| `av1_iidentity64_c` | Iidentity-64 |

### Quantization Functions

| Function | Purpose |
|----------|---------|
| `svt_aom_quantize_inv_quantize` | Full quantize+dequantize with RDOQ support |
| `svt_aom_quantize_inv_quantize_light` | Lightweight quantize+dequantize (no RDOQ, luma only context) |
| `svt_aom_quantize_b` | RTCD dead-zone quantizer (dispatches to SIMD) |
| `svt_av1_quantize_b_qm` | RTCD dead-zone quantizer with QM support |
| `svt_av1_quantize_fp` | RTCD full-precision quantizer |
| `svt_av1_quantize_fp_qm` | RTCD full-precision quantizer with QM |
| `svt_aom_highbd_quantize_b` | High bit depth dead-zone quantizer |
| `svt_av1_highbd_quantize_b_qm` | High bit depth dead-zone with QM |
| `svt_av1_highbd_quantize_fp` | High bit depth full-precision |
| `svt_av1_highbd_quantize_fp_qm` | High bit depth full-precision with QM |
| `svt_av1_quantize_fp_32x32` | Specialized FP quantizer for 32x32 (log_scale=1) |
| `svt_av1_quantize_fp_64x64` | Specialized FP quantizer for 64x64 (log_scale=2) |
| `svt_av1_optimize_b` | RDOQ: rate-distortion coefficient optimization |
| `svt_fast_optimize_b` | Fast RDOQ: simple EOB optimization |
| `svt_av1_compute_cul_level` | Compute cumulative level + DC sign for entropy context |
| `svt_aom_dc_quant_qtx` | qindex -> DC quantizer step lookup |
| `svt_aom_ac_quant_qtx` | qindex -> AC quantizer step lookup |
| `svt_aom_invert_quant` | Compute fixed-point reciprocal of dequant value |
| `svt_aom_get_qzbin_factor` | Get dead-zone bin factor |

### FFT Functions

| Function | Purpose |
|----------|---------|
| `svt_aom_fft{2,4,8,16,32}x{2,4,8,16,32}_float_c` | 2D forward FFT |
| `svt_aom_ifft{2,4,8,16,32}x{2,4,8,16,32}_float_c` | 2D inverse FFT |
| `svt_aom_fft1d_{4,8,16,32}_float` | 1D forward FFT |
| `svt_aom_fft_2d_gen` | Generic 2D FFT generator |
| `svt_aom_ifft_2d_gen` | Generic 2D IFFT generator |

### Coefficient / Scan Functions

| Function | Purpose |
|----------|---------|
| `get_scan_order` | Get scan order for (tx_size, tx_type) pair |
| `get_lower_levels_ctx` | Compute coefficient significance context |
| `get_br_ctx` | Compute base-range context |
| `get_nz_mag` | Compute neighbor magnitude sum |
| `av1_get_max_eob` | Get maximum EOB for a TX size |
| `av1_get_tx_scale` | Get log_scale for a TX size |

### Utility Functions

| Function | Purpose |
|----------|---------|
| `svt_av1_round_shift_array_c` | Apply round-shift to entire array (positive=right, negative=left) |
| `half_btf` | Butterfly: `round((w0*in0 + w1*in1), bit)` |
| `round_shift` | `(value + (1 << (bit-1))) >> bit` |
| `get_rect_tx_log_ratio` | Returns -2, -1, 0, 1, or 2 for the col:row ratio |
| `svt_handle_transform64x64` | Zero high-freq coefficients and repack 64x64 -> 32x32 |
| `energy_computation` | Sum of squared coefficients in a region |

## Dependencies

### Inputs Required
- **Residual buffer**: `int16_t*`, source - prediction difference, stride may differ from width
- **Bit depth**: 8, 10, or 12 (affects quantizer tables, transform precision)
- **QIndex**: 0-255, per-frame or per-segment, may have delta-q adjustments
- **Transform type**: selected by mode decision (tx_type_search)
- **Transform size**: determined by block partition and TX depth

### Outputs Produced
- **Coefficient buffer**: `int32_t*`, quantized coefficients in scan order
- **Reconstructed coefficient buffer**: `int32_t*`, dequantized coefficients
- **EOB**: end-of-block position (last non-zero coefficient in scan order)
- **Three-quad energy**: energy in zeroed-out regions (for 64-point transforms)
- **CUL level**: cumulative absolute level + DC sign context

### Struct Dependencies
- `PictureControlSet`: frame-level parameters (qindex, delta-q, QM settings)
- `ModeDecisionContext`: RDOQ controls, rate estimation tables, block geometry
- `EncodeContext`: pre-computed quant/dequant tables for all qindex values
- `Txfm2dFlipCfg`: populated by `svt_aom_transform_config` or `svt_av1_get_inv_txfm_cfg`
- `MacroblockPlane`: per-TU quantization parameters

### Table Dependencies
- `fwd_txfm_shift_ls[19]`: forward shift arrays
- `svt_aom_inv_txfm_shift_ls[19]`: inverse shift arrays
- `fwd_cos_bit_col[5][5]`, `fwd_cos_bit_row[5][5]`: forward cosine bit precision
- `inv_cos_bit_col[5][5]`, `inv_cos_bit_row[5][5]`: inverse cosine bit precision (all 12)
- `av1_txfm_type_ls[5][4]`: maps (size_idx, type_1d) -> TxfmType
- `av1_txfm_stage_num_list[14]`: stage count per TxfmType
- `vtx_tab[16]`, `htx_tab[16]`: 2D-to-1D type decomposition
- `eb_av1_scan_orders[19][3]`: scan orders for all sizes and 3 patterns
- `dc_qlookup_*[256]`, `ac_qlookup_*[256]`: qindex -> quantizer step
- `wt_matrix_ref[NUM_QM_LEVELS][2][QM_TOTAL_SIZE]`: quantization matrices

## SIMD Functions

The following functions have SIMD implementations dispatched via RTCD function pointers. The C reference implementations are the normative algorithm; SIMD implementations must produce identical results.

### Forward Transform (AVX2/SSE4.1)

All 19 sizes x 3 shapes (default, N2, N4) have AVX2 SIMD implementations:

| RTCD Function | Sizes |
|---------------|-------|
| `svt_av1_fwd_txfm2d_{size}` | 4x4, 8x8, 16x16, 32x32, 64x64, 4x8, 8x4, 8x16, 16x8, 16x32, 32x16, 32x64, 64x32, 4x16, 16x4, 8x32, 32x8, 16x64, 64x16 |
| `svt_av1_fwd_txfm2d_{size}_N2` | Same 19 sizes |
| `svt_av1_fwd_txfm2d_{size}_N4` | Same 19 sizes |

Source files:
- `Source/Lib/ASM_SSE4_1/highbd_fwd_txfm_sse4.c`
- `Source/Lib/ASM_AVX2/highbd_fwd_txfm_avx2.c`
- `Source/Lib/ASM_AVX2/transforms_intrin_avx2.c`

### Inverse Transform (AVX2/SSE4.1/SSSE3/NEON/AVX512)

| RTCD Pattern | Source Files |
|--------------|-------------|
| `svt_av1_inv_txfm2d_add_{size}_c` | `inv_transforms.c` |
| SSE4.1 | `ASM_SSE4_1/highbd_inv_txfm_sse4.c` |
| SSSE3 | `ASM_SSSE3/av1_inv_txfm_ssse3.c` |
| AVX2 | `ASM_AVX2/av1_inv_txfm_avx2.c`, `ASM_AVX2/highbd_inv_txfm_avx2.c` |
| AVX512 | `ASM_AVX512/highbd_inv_txfm_avx512.c` |
| NEON | `ASM_NEON/highbd_inv_txfm_neon.c`, `ASM_NEON/av1_inv_txfm_neon.c` |

### Quantization (AVX2/SSE4.1)

| RTCD Function | Source |
|---------------|--------|
| `svt_aom_quantize_b` | `ASM_AVX2/av1_quantize_avx2.c`, `ASM_SSE4_1/av1_quantize_sse4_1.c` |
| `svt_av1_quantize_b_qm` | Same |
| `svt_aom_highbd_quantize_b` | `ASM_AVX2/highbd_quantize_intrin_avx2.c` |
| `svt_av1_highbd_quantize_b_qm` | Same |
| `svt_av1_quantize_fp` | `ASM_AVX2/av1_quantize_avx2.c` |
| `svt_av1_quantize_fp_qm` | Same |
| `svt_av1_quantize_fp_32x32` | Same |
| `svt_av1_quantize_fp_64x64` | Same |
| `svt_av1_highbd_quantize_fp` | `ASM_AVX2/highbd_quantize_intrin_avx2.c` |
| `svt_av1_highbd_quantize_fp_qm` | Same |

### FFT (SSE2/AVX2)

| RTCD Function | Source |
|---------------|--------|
| `svt_aom_fft{N}x{N}_float` | `ASM_SSE2/fft_sse2.c`, `ASM_AVX2/fft_avx2.c` |
| `svt_aom_ifft{N}x{N}_float` | Same |

### Other SIMD

| Function | Purpose | Source |
|----------|---------|--------|
| `svt_av1_txb_init_levels` | Initialize coefficient level buffer for context | Various ASM dirs |
| `svt_av1_compute_cul_level` | CUL level computation | Various ASM dirs |
| `svt_handle_transform64x64` | 64x64 coefficient repacking | AVX2 |
| `svt_av1_round_shift_array` | Array round-shift | Various ASM dirs |
