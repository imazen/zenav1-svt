# Entropy Coding

## Overview

The entropy coding subsystem converts encoder decisions (partition types, prediction modes, transform types, motion vectors, quantized coefficients, etc.) into an AV1-conformant bitstream. It operates at the frame level and is the final encoding stage before packetization.

SVT-AV1 uses two distinct bitstream writing mechanisms:

1. **Arithmetic coding** (multi-symbol range coder) -- for all syntax elements within tile data. Symbols are coded using CDF (Cumulative Distribution Function) tables stored in the `FRAME_CONTEXT`. After each symbol is coded, the CDF may be updated via backward adaptation, allowing the probability model to track local statistics.

2. **Raw bit writing** (`AomWriteBitBuffer`) -- for frame headers, sequence headers, and other fixed-format OBU (Open Bitstream Unit) syntax. These are written as literal bits without arithmetic coding.

The encoding pipeline for a single frame:
1. Reset entropy coder for each tile (copy CDFs from reference frame or initialize defaults)
2. Reset neighbor arrays for context derivation
3. For each superblock in raster order within each tile, call `svt_aom_write_modes_sb()` which recurses through the partition tree
4. For each coding block, `write_modes_b()` encodes all syntax elements using arithmetic coding
5. Finalize each tile's bitstream with `svt_aom_encode_slice_finish()`
6. Write the frame header and tile group headers using raw bit writing
7. Package everything into OBUs

## Source Files

| File | Lines | Role |
|------|-------|------|
| `Source/Lib/Codec/entropy_coding.c` | 5599 | Main entropy coding: all syntax element writers, frame/sequence header OBU construction, coefficient coding, partition/mode/MV writing |
| `Source/Lib/Codec/entropy_coding.h` | 235 | Public API declarations for entropy coding functions |
| `Source/Lib/Codec/bitstream_unit.c` | 424 | Arithmetic range coder engine (`OdEcEnc`), raw bit buffer operations, byte-level I/O |
| `Source/Lib/Codec/bitstream_unit.h` | 389 | Encoder state structures (`OdEcEnc`, `AomWriter`, `AomWriteBitBuffer`), inline write functions (`aom_write_symbol`, `aom_write_cdf`, `aom_write_literal`) |
| `Source/Lib/Codec/cabac_context_model.c` | 2878 | Default CDF tables for all syntax elements, transform type mapping tables |
| `Source/Lib/Codec/cabac_context_model.h` | ~760 | `FRAME_CONTEXT` (all CDF arrays), `NmvContext`, `update_cdf()`, constants for context counts |
| `Source/Lib/Codec/ec_process.c` | 266 | Entropy coding process kernel (threading), reset/init, tile-level encode loop |
| `Source/Lib/Codec/ec_process.h` | 74 | `EntropyCodingContext` structure, kernel entry point |
| `Source/Lib/Codec/ec_object.h` | 58 | `EntropyCoder`, `Bitstream`, `EntropyTileInfo` structures |
| `Source/Lib/Codec/ac_bias.c` | 211 | Psychovisual distortion ("AC Bias") -- not directly entropy coding, but used for rate-distortion adjustments that influence coding decisions |
| `Source/Lib/Codec/ac_bias.h` | 33 | AC bias function declarations |

## Test Coverage

| Test File | Tests | What Is Tested |
|-----------|-------|----------------|
| `test/BitstreamWriterTest.cc` | `write_bits_random`, `write_literal_extreme_int`, `write_symbol_no_update`, `write_symbol_with_update` | Round-trip encode/decode of arithmetic-coded bits with various probability distributions; literal writing of extreme int values; symbol coding with and without CDF update |
| `test/AdaptiveScanTest.cc` | `scan_tables_test`, `CopyMiMapGridTest` | Verification of scan table ordering (zig-zag, vertical, horizontal) for all tx sizes and types; SIMD vs. C reference for `svt_copy_mi_map_grid` |

## Data Structures

### OdEcEnc -- Arithmetic Encoder State

The core range coder state, defined in `bitstream_unit.h`:

```c
typedef struct OdEcEnc {
    unsigned char* buf;      // Output buffer for encoded bytes
    uint32_t       storage;  // Allocated buffer size
    uint32_t       offs;     // Current write offset in buf
    OdEcWindow     low;      // Low end of current range (uint64_t)
    uint16_t       rng;      // Current range size (invariant: 32768 <= rng < 65536)
    int16_t        cnt;      // Bit count, initialized to -9
    int            error;    // Nonzero on allocation failure
} OdEcEnc;
```

`OdEcWindow` is `uint64_t`, giving 64 bits of precision for the `low` accumulator. The encoder flushes bytes when `cnt` reaches 40 (i.e., `s >= 40` where `s = cnt + d`, with `d` being the number of leading zeros in `rng`).

### AomWriter -- High-Level Writer

Wraps `OdEcEnc` with buffer management:

```c
typedef struct AomWriter {
    unsigned int         pos;            // Bytes written after finalization
    uint8_t*             buffer;         // Output buffer pointer
    uint32_t             buffer_size;    // Buffer capacity
    OutputBitstreamUnit* buffer_parent;  // Parent for reallocation
    OdEcEnc              ec;             // The arithmetic coder
    uint8_t              allow_update_cdf; // Whether to perform backward adaptation
} AomWriter;
```

### AomWriteBitBuffer -- Raw Bit Writer

For frame/sequence headers (non-arithmetic-coded portions):

```c
typedef struct AomWriteBitBuffer {
    uint8_t* bit_buffer;
    uint32_t bit_offset;
} AomWriteBitBuffer;
```

### FRAME_CONTEXT (FrameContexts) -- CDF Tables

The `FRAME_CONTEXT` structure (~748 lines in `cabac_context_model.h`) holds all CDF arrays used for arithmetic coding. Key groups:

**Coefficient coding CDFs:**
- `txb_skip_cdf[TX_SIZES][TXB_SKIP_CONTEXTS(13)][CDF_SIZE(2)]` -- whether a transform block has all-zero coefficients
- `eob_flag_cdf{16,32,64,128,256,512,1024}[PLANE_TYPES][2][CDF_SIZE(N)]` -- end-of-block position, size-specific
- `eob_extra_cdf[TX_SIZES][PLANE_TYPES][EOB_COEF_CONTEXTS(9)][CDF_SIZE(2)]` -- extra bits for EOB
- `coeff_base_eob_cdf[TX_SIZES][PLANE_TYPES][SIG_COEF_CONTEXTS_EOB(4)][CDF_SIZE(3)]` -- base level at EOB position
- `coeff_base_cdf[TX_SIZES][PLANE_TYPES][SIG_COEF_CONTEXTS(42)][CDF_SIZE(4)]` -- base coefficient level
- `coeff_br_cdf[TX_SIZES][PLANE_TYPES][LEVEL_CONTEXTS(21)][CDF_SIZE(BR_CDF_SIZE=4)]` -- base range (higher levels)
- `dc_sign_cdf[PLANE_TYPES][DC_SIGN_CONTEXTS(3)][CDF_SIZE(2)]` -- DC coefficient sign

**Mode CDFs:**
- `partition_cdf[PARTITION_CONTEXTS][CDF_SIZE(EXT_PARTITION_TYPES)]`
- `kf_y_cdf[KF_MODE_CONTEXTS(5)][KF_MODE_CONTEXTS(5)][CDF_SIZE(INTRA_MODES)]` -- keyframe luma mode
- `y_mode_cdf[BlockSize_GROUPS(4)][CDF_SIZE(INTRA_MODES)]` -- inter-frame intra luma mode
- `uv_mode_cdf[CFL_ALLOWED_TYPES][INTRA_MODES][CDF_SIZE(UV_INTRA_MODES)]`
- `angle_delta_cdf[DIRECTIONAL_MODES][CDF_SIZE(2 * MAX_ANGLE_DELTA + 1)]`
- `filter_intra_cdfs[BLOCK_SIZES_ALL][CDF_SIZE(2)]`
- `filter_intra_mode_cdf[CDF_SIZE(FILTER_INTRA_MODES)]`

**Inter prediction CDFs:**
- `newmv_cdf[NEWMV_MODE_CONTEXTS][CDF_SIZE(2)]`
- `zeromv_cdf[GLOBALMV_MODE_CONTEXTS][CDF_SIZE(2)]`
- `refmv_cdf[REFMV_MODE_CONTEXTS][CDF_SIZE(2)]`
- `drl_cdf[DRL_MODE_CONTEXTS][CDF_SIZE(2)]`
- `inter_compound_mode_cdf[INTER_MODE_CONTEXTS][CDF_SIZE(INTER_COMPOUND_MODES)]`
- `intra_inter_cdf[INTRA_INTER_CONTEXTS][CDF_SIZE(2)]`
- `skip_cdfs[SKIP_CONTEXTS][CDF_SIZE(2)]`
- `skip_mode_cdfs[SKIP_CONTEXTS][CDF_SIZE(2)]`

**Reference frame CDFs:**
- `comp_inter_cdf[COMP_INTER_CONTEXTS][CDF_SIZE(2)]`
- `single_ref_cdf[REF_CONTEXTS][SINGLE_REFS - 1][CDF_SIZE(2)]`
- `comp_ref_cdf[REF_CONTEXTS][FWD_REFS - 1][CDF_SIZE(2)]`
- `comp_bwdref_cdf[REF_CONTEXTS][BWD_REFS - 1][CDF_SIZE(2)]`
- `comp_ref_type_cdf[COMP_REF_TYPE_CONTEXTS][CDF_SIZE(2)]`
- `uni_comp_ref_cdf[UNI_COMP_REF_CONTEXTS][UNIDIR_COMP_REFS - 1][CDF_SIZE(2)]`

**Transform CDFs:**
- `tx_size_cdf[MAX_TX_CATS][TX_SIZE_CONTEXTS][CDF_SIZE(MAX_TX_DEPTH + 1)]`
- `txfm_partition_cdf[TXFM_PARTITION_CONTEXTS][CDF_SIZE(2)]`
- `intra_ext_tx_cdf[EXT_TX_SETS_INTRA][EXT_TX_SIZES][INTRA_MODES][CDF_SIZE(TX_TYPES)]`
- `inter_ext_tx_cdf[EXT_TX_SETS_INTER][EXT_TX_SIZES][CDF_SIZE(TX_TYPES)]`

**Motion vector CDFs** (`NmvContext`):
- `joints_cdf[CDF_SIZE(MV_JOINTS=4)]`
- Per-component (`NmvComponent`): `sign_cdf`, `classes_cdf[CDF_SIZE(MV_CLASSES=11)]`, `class0_cdf`, `class0_fp_cdf`, `fp_cdf`, `class0_hp_cdf`, `hp_cdf`, `bits_cdf[MV_OFFSET_BITS][CDF_SIZE(2)]`

**Other CDFs:**
- `switchable_interp_cdf`, `motion_mode_cdf`, `obmc_cdf`, `compound_type_cdf`, `wedge_idx_cdf`, `interintra_cdf`, `palette_*_cdf`, `delta_q_cdf`, `delta_lf_cdf`, `cfl_sign_cdf`, `cfl_alpha_cdf`, `switchable_restore_cdf`, `wiener_restore_cdf`, `sgrproj_restore_cdf`, segmentation CDFs

### CDF Format

CDFs are stored as arrays of `uint16_t` (typedef `AomCdfProb`). They use an "inverse CDF" (iCDF) representation where:
```
AOM_ICDF(x) = CDF_PROB_TOP - x    // CDF_PROB_TOP = 32768 (1 << 15)
```

Each CDF array has `nsymbs + 1` entries: `nsymbs` iCDF values followed by a count field used for adaptation rate control. The count is stored in `cdf[nsymbs]`.

The macro `CDF_SIZE(x) = (x) + 1` accounts for this extra count slot.

### NmvContext -- Motion Vector Context

```c
typedef struct NmvContext {
    AomCdfProb   joints_cdf[CDF_SIZE(MV_JOINTS)];
    NmvComponent comps[2];  // [0] = vertical, [1] = horizontal
} NmvContext;

typedef struct NmvComponent {
    AomCdfProb classes_cdf[CDF_SIZE(MV_CLASSES)];
    AomCdfProb class0_fp_cdf[CLASS0_SIZE][CDF_SIZE(MV_FP_SIZE)];
    AomCdfProb fp_cdf[CDF_SIZE(MV_FP_SIZE)];
    AomCdfProb sign_cdf[CDF_SIZE(2)];
    AomCdfProb class0_hp_cdf[CDF_SIZE(2)];
    AomCdfProb hp_cdf[CDF_SIZE(2)];
    AomCdfProb class0_cdf[CDF_SIZE(CLASS0_SIZE)];
    AomCdfProb bits_cdf[MV_OFFSET_BITS][CDF_SIZE(2)];
} NmvComponent;
```

### EntropyCoder

```c
typedef struct EntropyCoder {
    FRAME_CONTEXT* fc;               // Per-tile CDF tables
    AomWriter      ec_writer;        // Arithmetic coder
    EbPtr          ec_output_bitstream_ptr; // Output buffer
    uint64_t       ec_frame_size;    // Accumulated frame size
} EntropyCoder;
```

### EntropyCodingContext

Per-thread state for the entropy coding process:

```c
typedef struct EntropyCodingContext {
    EbFifo*      enc_dec_input_fifo_ptr;
    EbFifo*      entropy_coding_output_fifo_ptr;
    bool         is_16bit;
    int32_t      coded_area_sb;
    int32_t      coded_area_sb_uv;
    TOKENEXTRA*  tok;                // Palette token stream pointer
    MbModeInfo*  mbmi;               // Current block's mode info
    bool         cdef_transmitted[4]; // CDEF strength transmission tracking
    WienerInfo   wiener_info[MAX_PLANES];   // Reference Wiener params for delta coding
    SgrprojInfo  sgrproj_info[MAX_PLANES];  // Reference Sgrproj params for delta coding
} EntropyCodingContext;
```

## Algorithms

### Arithmetic Coding Engine

The arithmetic coder is a multi-symbol range coder based on the Daala entropy coding design (Martin 1979, Moffat-Neal-Witten 1998). All arithmetic-coded syntax elements go through this engine.

#### State

- `low` (64-bit): accumulator for the lower bound of the current interval
- `rng` (16-bit): current range, maintained in [32768, 65535] after normalization
- `cnt` (16-bit signed): bit counter, initialized to -9
- `buf` / `offs`: output byte buffer and current write position

#### Encoding a CDF Symbol (`svt_od_ec_encode_cdf_q15`)

Given symbol `s` in alphabet of `nsyms` symbols, with iCDF table `icdf`:

1. Look up `fl = icdf[s-1]` (or `OD_ICDF(0)` if `s == 0`) and `fh = icdf[s]`
2. Call `svt_od_ec_encode_q15(enc, fl, fh, s, nsyms)` which:
   - Computes subranges `u` and `v` using the formula:
     ```
     N = nsyms - 1
     u = ((r >> 8) * (fl >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB * (N - (s - 1))
     v = ((r >> 8) * (fh >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB * (N - s)
     ```
   - `EC_PROB_SHIFT = 6`, `EC_MIN_PROB = 4`
   - Updates: `low += r - u`, `r = u - v` (or `r -= v` if first symbol)
3. Normalize

#### Encoding a Boolean (`svt_od_ec_encode_bool_q15`)

For binary decisions with probability `f` (in Q15, i.e., probability of 1 is `f/32768`):

```
v = ((r >> 8) * (f >> EC_PROB_SHIFT) >> (7 - EC_PROB_SHIFT)) + EC_MIN_PROB
if (val) low += r - v
r = val ? v : r - v
normalize
```

The `aom_write()` wrapper converts an 8-bit probability `prob` to Q15: `p = (0x7FFFFF - (prob << 15) + prob) >> 8`.

#### Renormalization (`svt_od_ec_enc_normalize`)

After each symbol:

1. Compute `d = 16 - floor(log2(rng))` (number of leading zeros in 16-bit `rng`)
2. Compute `s = cnt + d`
3. If `s >= 40`: flush bytes to the output buffer
   - Calculate how many bytes are ready: `num_bytes_ready = (s >> 3) + 1`
   - Extract the ready bytes from `low`, handle carry propagation
   - Write bytes via `write_enc_data_to_out_buf()` which converts to big-endian and propagates carries backward
   - Update `offs` and adjust `cnt`
4. Shift: `low <<= d`, `rng <<= d`, `cnt = s` (or adjusted `s` after flush)

#### Carry Handling

Carries propagate backward through already-written bytes:

```c
static inline void propagate_carry_bwd(unsigned char* buf, uint32_t offs) {
    uint16_t carry = 1;
    do {
        uint16_t sum = (uint16_t)buf[offs] + 1;
        buf[offs--]  = (unsigned char)sum;
        carry        = sum >> 8;
    } while (carry);
}
```

The encoder writes bytes in big-endian order via `HToBE64()` and checks whether the carry bit is set in the output word. If so, it calls `propagate_carry_bwd()` on the preceding bytes.

#### Finalization (`svt_od_ec_enc_done`)

1. Round up `low` to the minimum value that ensures unambiguous decoding:
   ```
   m = 0x3FFF
   e = ((low + m) & ~m) | (m + 1)
   ```
2. Flush remaining bytes from `e` one byte at a time, propagating carries
3. Return pointer to the completed buffer and its size

#### Bit Counting (`svt_od_ec_enc_tell` / `svt_od_ec_enc_tell_frac`)

- `tell()` returns `(cnt + 10) + offs * 8` -- the total number of bits used
- `tell_frac()` adds sub-bit precision using `OD_BITRES = 3` (1/8th bit resolution), computed by squaring `rng` repeatedly

### CDF Update (Backward Adaptation)

After coding each symbol via `aom_write_symbol()`, if `allow_update_cdf` is true, the CDF is updated:

```c
static INLINE void update_cdf(AomCdfProb* cdf, int8_t val, int nsymbs) {
    const int count = cdf[nsymbs];   // adaptation counter
    // rate = 4 + (count >> 4) + (nsymbs > 3)
    // This approximates: 3 + (count > 15) + (count > 31) + min(floor(log2(nsymbs)), 2)
    const int rate = 4 + (count >> 4) + (nsymbs > 3);

    for (int i = 0; i < nsymbs - 1; i++) {
        if (i < val)
            cdf[i] += (CDF_PROB_TOP - cdf[i]) >> rate;
        else
            cdf[i] -= cdf[i] >> rate;
    }
    cdf[nsymbs] += (count < 32);  // increment counter, saturates at 32
}
```

Key properties:
- The adaptation rate starts fast (rate=4 or 5) and slows down as more symbols are seen (rate up to 7)
- The counter saturates at 32 (5 bits), after which the rate is fixed
- For binary symbols (`nsymbs=2`): rate ranges from 4 to 6
- For larger alphabets (`nsymbs>3`): rate ranges from 5 to 7
- `allow_update_cdf` is false when `disable_cdf_update` is set in the frame header or in large-scale tile mode

### Context Selection

Context derivation uses neighbor information from `MacroBlockD` and various `NeighborArrayUnit` structures. The general pattern:

1. Get above (top) and left neighbor mode info from `xd->above_mbmi` and `xd->left_mbmi`
2. Extract relevant properties from neighbors (e.g., skip flag, reference frames, prediction mode)
3. Combine into a context index used to select the appropriate CDF

### Subexponential Coding

For global motion parameters, Wiener/Sgrproj filter parameters, and similar syntax elements, SVT-AV1 uses subexponential codes:

- **`write_primitive_quniform(n, v)`**: Quasi-uniform code for `v` in `[0, n-1]`. Uses `l = floor(log2(n-1)) + 1` bits for the first `m = (1 << l) - n` values, `l` bits for the rest.

- **`write_primitive_subexpfin(n, k, v)`**: Finite subexponential code for `v` in `[0, n-1]` with parameter `k`. Codes small values with `k` bits, medium values with increasing bit widths, and large values with quasi-uniform codes.

- **`write_primitive_refsubexpfin(n, k, ref, v)`**: Recenters `v` around `ref` before applying subexponential coding. Used for delta-coding parameters relative to a reference.

### Exponential-Golomb Coding

For large coefficient residuals beyond the base range:

```c
static void write_golomb(AomWriter* w, int32_t level) {
    const int32_t x = level + 1;
    const uint32_t length = floor(log2(x)) + 1;
    // Write (length-1) zeros, then the binary representation of x
    for (uint32_t i = 0; i < length - 1; ++i) aom_write_bit(w, 0);
    for (int32_t i = length - 1; i >= 0; --i) aom_write_bit(w, (x >> i) & 0x01);
}
```

## Syntax Element Coding

### Partition Type

**Function:** `encode_partition_av1()`

Partition is only coded for blocks >= BLOCK_8X8. The context is derived from neighbor partition information:

1. Read above and left partition context from `partition_context_na`
2. Compute `bsl = log2(mi_size_wide[bsize]) - log2(mi_size_wide[BLOCK_8X8])`
3. Extract single-bit indicators: `above = (above_ctx >> bsl) & 1`, `left = (left_ctx >> bsl) & 1`
4. Context index: `(left * 2 + above) + bsl * PARTITION_PLOFFSET`

Three cases:
- **Both rows and cols available:** Write full partition symbol using `partition_cdf[ctx]` with `partition_cdf_length(bsize)` symbols
- **Only cols available (no rows):** Gather vertical-alike partitions into binary CDF, write binary decision (PARTITION_SPLIT vs. not)
- **Only rows available (no cols):** Gather horizontal-alike partitions into binary CDF, write binary decision

Partition types: NONE, HORZ, VERT, SPLIT, HORZ_A, HORZ_B, VERT_A, VERT_B, HORZ_4, VERT_4.

### Skip Coefficient Flag

**Function:** `encode_skip_coeff_av1()`

```
context = above_skip + left_skip  (each 0 or 1, so context in [0,2])
aom_write_symbol(ec_writer, skip_coeff, frame_context->skip_cdfs[ctx], 2)
```

### Skip Mode Flag

**Function:** `encode_skip_mode_av1()`

Only coded when `skip_mode_flag` is enabled at the frame level and block supports compound references.

```
context = above_skip_mode + left_skip_mode  (context in [0,2])
aom_write_symbol(ec_writer, skip_mode, frame_context->skip_mode_cdfs[ctx], 2)
```

### Intra/Inter Decision

**Function:** `write_is_inter()`

Context from `svt_av1_get_intra_inter_context()`:
- Both neighbors available: `left_intra && above_intra ? 3 : left_intra || above_intra` (context 0-3)
- Only one neighbor: based on whether that neighbor is intra
- No neighbors: context 0

```
aom_write_symbol(ec_writer, is_inter, frame_context->intra_inter_cdf[ctx], 2)
```

### Intra Luma Mode

**Key frame** (`encode_intra_luma_mode_kf_av1`):
- Context: `kf_y_cdf[above_mode_ctx][left_mode_ctx]` where contexts come from `intra_mode_context[]` lookup
- Symbol: luma mode (0 to INTRA_MODES-1)
- If directional mode and bsize >= BLOCK_8X8: also code angle delta

**Non-key frame** (`encode_intra_luma_mode_nonkey_av1`):
- Context: `y_mode_cdf[size_group_lookup[bsize]]`
- Symbol: luma mode
- Same angle delta coding as keyframe

### Intra Chroma Mode

**Function:** `encode_intra_chroma_mode_av1()`

- CDF: `uv_mode_cdf[cfl_allowed][luma_mode]`
- Number of symbols: `UV_INTRA_MODES - !cfl_allowed` (CFL_PRED is only available when CFL is allowed)
- If UV_CFL_PRED: also code CFL alpha parameters via `write_cfl_alphas()`
- If directional UV mode and bsize >= BLOCK_8X8: code angle delta

### Filter Intra

Coded when allowed (based on sequence header, block size, palette, and luma mode):
1. Write boolean: `filter_intra_mode != FILTER_INTRA_MODES` using `filter_intra_cdfs[bsize]`
2. If filter intra is used: write mode index using `filter_intra_mode_cdf` with `FILTER_INTRA_MODES` symbols

### Inter Prediction Mode

**Single reference modes** (`write_inter_mode`):

A 3-step binary tree:
1. `NEWMV` vs. not: `newmv_cdf[newmv_ctx]` (context from lower bits of `mode_ctx`)
2. If not NEWMV: `GLOBALMV` vs. not: `zeromv_cdf[zeromv_ctx]` (context from `GLOBALMV_OFFSET` bits)
3. If not GLOBALMV: `NEARESTMV` vs. `NEARMV`: `refmv_cdf[refmv_ctx]` (context from `REFMV_OFFSET` bits)

**Compound modes** (`write_inter_compound_mode`):

Single symbol from `inter_compound_mode_cdf[mode_ctx]` with `INTER_COMPOUND_MODES` alternatives (NEAREST_NEARESTMV, NEAR_NEARMV, NEW_NEARESTMV, etc.).

### DRL (Dynamic Reference List) Index

**Function:** `write_drl_idx()`

For NEWMV/NEW_NEWMV: iterate up to 2 DRL candidates, coding binary decisions using `drl_cdf[drl_ctx]`.

For NEAR modes: iterate candidates 1-2, coding binary decisions.

### Reference Frames

**Function:** `write_ref_frames()`

First, if REFERENCE_MODE_SELECT, code compound vs. single using `comp_inter_cdf`.

**Compound references** -- binary tree:
1. Code reference type (uni-directional vs. bi-directional) using `comp_ref_type_cdf`
2. For uni-directional: code forward reference choices using `uni_comp_ref_p`, `uni_comp_ref_p1`, `uni_comp_ref_p2`
3. For bi-directional: code forward ref using `comp_ref_p`, `comp_ref_p1`/`comp_ref_p2`, and backward ref using `comp_bwdref_p`, `comp_bwdref_p1`

**Single references** -- binary tree with 6 decision points (`single_ref_p1` through `single_ref_p6`):
1. Forward vs. backward (`p1`)
2. If backward: ALTREF vs. not (`p2`), then ALTREF2 vs. BWDREF (`p6`)
3. If forward: LAST3/GOLDEN vs. LAST/LAST2 (`p3`), then LAST vs. LAST2 (`p4`) or LAST3 vs. GOLDEN (`p5`)

### Motion Vectors

**Function:** `svt_av1_encode_mv()` -> `encode_mv_component()`

MV is coded as a differential `(mv - ref_mv)` for each component:

1. **Joint type**: `MV_JOINT_ZERO`, `MV_JOINT_HNZVZ`, `MV_JOINT_HZVNZ`, `MV_JOINT_HNZVNZ` using `joints_cdf`
2. For each nonzero component:
   - **Sign**: `sign_cdf`
   - **Class** (magnitude range): `classes_cdf` with MV_CLASSES=11 symbols
   - **Integer bits**:
     - Class 0: `class0_cdf` (binary)
     - Class 1+: `n = class + CLASS0_BITS - 1` bits using `bits_cdf[i]`
   - **Fractional bits** (if not integer-MV): `class0_fp_cdf[d]` or `fp_cdf` with MV_FP_SIZE=4 symbols
   - **High precision bit** (if high-precision MV): `class0_hp_cdf` or `hp_cdf`

MV class ranges (integer pel): Class 0: (0,2], Class 1: (2,4], ..., Class 10: (1024,2048].

### Transform Type

**Function:** `av1_write_tx_type()`

Only coded when `get_ext_tx_types() > 1` and `base_q_idx > 0`:
- Look up the extended transform set type based on tx_size, is_inter, reduced_tx_set
- Map the tx_type to an index using `av1_ext_tx_ind[tx_set_type][tx_type]`
- For inter: use `inter_ext_tx_cdf[eset][square_tx_size]`
- For intra: use `intra_ext_tx_cdf[eset][square_tx_size][intra_dir]` (with filter_intra mapping)

### Transform Size

**Function:** `code_tx_size()` -> `write_tx_size_vartx()` / `write_selected_tx_size()`

When `TX_MODE_SELECT`:
- For inter blocks with variable transform sizes: recursively code the transform partition tree using `txfm_partition_cdf[ctx]`
- For intra blocks: code the selected depth using `tx_size_cdf[tx_size_cat][tx_size_ctx]`

Transform partition context (`txfm_partition_context`) is derived from above and left TXFM_CONTEXT arrays.

### Coefficient Coding

**Function:** `av1_write_coeffs_txb_1d()` -- the most complex syntax element

This is the core of the entropy coder. For each transform block:

#### Step 1: TXB Skip
```
aom_write_symbol(ec_writer, eob == 0, txb_skip_cdf[txs_ctx][txb_skip_ctx], 2)
```
Context `txb_skip_ctx` from `svt_aom_get_txb_ctx()` using neighbor coefficient levels.

If `eob == 0`, done (all zeros).

#### Step 2: Initialize Coefficient Level Buffer
```
svt_av1_txb_init_levels(coeff_buffer, width, height, levels)
```

#### Step 3: Transform Type (luma only)
Write tx_type as described above.

#### Step 4: End-of-Block Position
1. Map eob to eob_pt (position token) using `eob_to_pos_small[]` or `eob_to_pos_large[]`
2. Select size-appropriate CDF (`eob_flag_cdf16` through `eob_flag_cdf1024`)
3. Context: tx class (2D vs. 1D) gives `eob_multi_ctx`
4. Write `eob_pt - 1` using the selected CDF
5. If extra bits needed (`eob_offset_bits[eob_pt] > 0`):
   - First extra bit via `eob_extra_cdf[txs_ctx][component_type][eob_ctx]`
   - Remaining extra bits as raw bits

#### Step 5: Compute NZ Map Contexts
```
svt_av1_get_nz_map_contexts(levels, scan, eob, tx_size, tx_class, coeff_contexts)
```

#### Step 6: Coefficient Base Levels (reverse scan order)
For each coefficient from `eob-1` down to 0:
- At EOB position: code `min(level, 3) - 1` using `coeff_base_eob_cdf` (3 symbols)
- Other positions: code `min(level, 3)` using `coeff_base_cdf` (4 symbols, where 0 means zero coefficient)

#### Step 7: Base Range (higher levels)
If `level > NUM_BASE_LEVELS(2)`:
- Compute `base_range = level - 1 - NUM_BASE_LEVELS`
- Get BR context from `get_br_ctx(levels, pos, bwl, tx_class)`
- Code in chunks of `BR_CDF_SIZE - 1 = 3`: write `min(base_range - idx, 3)` using `coeff_br_cdf`
- If the chunk value is < 3, stop (no more range to code)
- Up to `COEFF_BASE_RANGE(12) / 3 = 4` chunks

#### Step 8: Signs and Golomb Residuals (forward scan order)
For each nonzero coefficient from position 0 to `eob-1`:
- DC sign: `dc_sign_cdf[component_type][dc_sign_ctx]`
- Non-DC sign: raw bit
- If `level > COEFF_BASE_RANGE + NUM_BASE_LEVELS`: write Golomb-coded residual

#### Step 9: Cumulative Level
Accumulate total coefficient level (capped at `COEFF_CONTEXT_MASK = 63`) and set DC sign for neighbor updates.

### Delta Q Index

**Function:** `av1_write_delta_q_index()`

1. Write `min(|delta_qindex|, DELTA_Q_SMALL)` using `delta_q_cdf`
2. If large: write `rem_bits - 1` (3 raw bits) and `|delta_qindex| - thr` (rem_bits raw bits)
3. If nonzero: write sign bit

### CDEF Strength

**Function:** `write_cdef()` (block-level)

Written at the first non-skip coding block in each 64x64 CDEF unit. The CDEF strength index is written as raw bits with width `cdef_bits`.

Tracking array `cdef_transmitted[4]` ensures each CDEF unit's strength is only written once per superblock.

### Segmentation

**Functions:** `write_segment_id()`, `write_inter_segment_id()`, `encode_segmentation()`

Frame-level segmentation parameters are written to the frame header using raw bits. Per-block segment IDs are coded using:
- Spatial prediction: `spatial_pred_seg_cdf[ctx]`
- Temporal prediction: `pred_cdf[ctx]` for the prediction flag, then `tree_cdf` for the residual

The `svt_av1_neg_interleave()` function recenters segment IDs around a predicted value.

### Loop Filter Parameters

**Function:** `encode_loopfilter()`

Written in the frame header as raw bits:
- `filter_level[0]`, `filter_level[1]` (6 bits each)
- If either nonzero: `filter_level_u`, `filter_level_v` (6 bits each)
- `sharpness_level` (3 bits)
- `mode_ref_delta_enabled` flag

### Loop Restoration

**Frame-level** (`encode_restoration_mode`): Raw bits selecting NONE/WIENER/SGRPROJ/SWITCHABLE for each plane, plus restoration unit size coding.

**Block-level** (`loop_restoration_write_sb_coeffs`):
- Restoration type: `switchable_restore_cdf`, `wiener_restore_cdf`, or `sgrproj_restore_cdf`
- Wiener filter coefficients: delta-coded using `write_primitive_refsubexpfin()`
- Sgrproj parameters: delta-coded using `write_primitive_refsubexpfin()`

### Interpolation Filter

**Function:** `write_mb_interp_filter()`

When frame-level filter is SWITCHABLE and interpolation is needed:
- For each direction (up to 2 if dual filter enabled):
  - Context from `svt_aom_get_pred_context_switchable_interp()`
  - Write filter using `switchable_interp_cdf[ctx]` with `SWITCHABLE_FILTERS` symbols

### Motion Mode

**Function:** `write_motion_mode()`

For blocks with `num_proj_ref > 0` and WARPED_CAUSAL allowed: code using `motion_mode_cdf[bsize]` with `MOTION_MODES` symbols.

For OBMC-only blocks: code using `obmc_cdf[bsize]` (binary).

### Compound Types

When the block uses compound prediction:
- **Compound group index**: `comp_group_idx_cdf[ctx]` -- selects between group A (distance-weighted/average) and group B (wedge/diffwtd)
- **Compound index** (group A): `compound_index_cdf[ctx]` -- selects between distance-weighted and average
- **Compound type** (group B): `compound_type_cdf[bsize]` -- wedge vs. diffwtd
- **Wedge index**: `wedge_idx_cdf[bsize]` with 16 symbols
- **Wedge sign**: raw bit
- **Diffwtd mask type**: raw literal

### Inter-Intra

When allowed:
1. Code `is_interintra_used` using `interintra_cdf[bsize_group]`
2. If used: code mode using `interintra_mode_cdf[bsize_group]`
3. If wedge available: code `use_wedge_interintra` using `wedge_interintra_cdf[bsize]`
4. If wedge: code index using `wedge_idx_cdf[bsize]`

### IntraBC

**Function:** `write_intrabc_info()`

Coded when allowed (key frame or intra-only with screen content tools):
1. Code `use_intrabc` flag using `intrabc_cdf`
2. If used: code the DV (displacement vector) using `svt_av1_encode_dv()` which uses `ndvc` (intraBC MV context)

### Palette Mode

Colors and indices are coded using specialized palette CDFs:
1. Y palette mode flag: `palette_y_mode_cdf[bsize_ctx][palette_ctx]`
2. UV palette mode flag: `palette_uv_mode_cdf[has_y_palette]`
3. Palette size: `palette_y_size_cdf[bsize_ctx]` / `palette_uv_size_cdf[bsize_ctx]`
4. Palette colors: delta-encoded
5. Color index map: tokenized via `svt_av1_tokenize_color_map()`, packed via `pack_map_tokens()` using `palette_y_color_index_cdf` / `palette_uv_color_index_cdf`

## Key Functions

### Arithmetic Coder Core

| Function | File | Purpose |
|----------|------|---------|
| `svt_od_ec_enc_init()` | `bitstream_unit.c` | Allocate and initialize encoder state |
| `svt_od_ec_enc_reset()` | `bitstream_unit.c` | Reset state: `rng=0x8000`, `cnt=-9`, `low=0`, `offs=0` |
| `svt_od_ec_encode_bool_q15()` | `bitstream_unit.c` | Encode binary symbol with Q15 probability |
| `svt_od_ec_encode_cdf_q15()` | `bitstream_unit.c` | Encode multi-symbol using iCDF table |
| `svt_od_ec_enc_normalize()` | `bitstream_unit.c` | Renormalize range and flush bytes |
| `svt_od_ec_enc_done()` | `bitstream_unit.c` | Finalize: flush remaining bits, return buffer |
| `svt_od_ec_enc_tell()` | `bitstream_unit.c` | Return bits used (integer precision) |
| `svt_od_ec_enc_tell_frac()` | `bitstream_unit.c` | Return bits used (1/8th bit precision) |
| `svt_od_ec_enc_clear()` | `bitstream_unit.c` | Free encoder buffer |

### High-Level Writers (inline in `bitstream_unit.h`)

| Function | Purpose |
|----------|---------|
| `aom_start_encode()` | Initialize AomWriter with an output buffer |
| `aom_stop_encode()` | Finalize, copy data to output, return bit count |
| `aom_write()` | Write boolean with 8-bit probability |
| `aom_write_bit()` | Write boolean with prob=128 (equiprobable) |
| `aom_write_literal()` | Write N raw bits MSB-first |
| `aom_write_cdf()` | Write symbol using CDF (no update) |
| `aom_write_symbol()` | Write symbol using CDF, then call `update_cdf()` if allowed |

### Raw Bit Writers

| Function | File | Purpose |
|----------|------|---------|
| `svt_aom_wb_write_bit()` | `entropy_coding.c` | Write single raw bit |
| `svt_aom_wb_write_literal()` | `entropy_coding.c` | Write N raw bits |
| `svt_aom_wb_write_inv_signed_literal()` | `entropy_coding.c` | Write signed value (magnitude then sign) |

### Block-Level Coding

| Function | File | Purpose |
|----------|------|---------|
| `svt_aom_write_modes_sb()` | `entropy_coding.c` | Recursively encode a superblock's partition tree |
| `write_modes_b()` | `entropy_coding.c` | Encode all syntax elements for a single coding block |
| `encode_partition_av1()` | `entropy_coding.c` | Encode partition type with neighbor context |
| `encode_skip_coeff_av1()` | `entropy_coding.c` | Encode skip_coeff flag |
| `encode_skip_mode_av1()` | `entropy_coding.c` | Encode skip_mode flag |
| `write_is_inter()` | `entropy_coding.c` | Encode intra/inter decision |
| `encode_intra_luma_mode_kf_av1()` | `entropy_coding.c` | Encode keyframe Y mode |
| `encode_intra_luma_mode_nonkey_av1()` | `entropy_coding.c` | Encode inter-frame intra Y mode |
| `encode_intra_chroma_mode_av1()` | `entropy_coding.c` | Encode UV mode |
| `write_inter_mode()` | `entropy_coding.c` | Encode single-ref inter mode (3-step tree) |
| `write_inter_compound_mode()` | `entropy_coding.c` | Encode compound inter mode |
| `write_drl_idx()` | `entropy_coding.c` | Encode DRL index for MV selection |
| `svt_av1_encode_mv()` | `entropy_coding.c` | Encode differential motion vector |
| `encode_mv_component()` | `entropy_coding.c` | Encode single MV component (sign, class, bits, frac, hp) |
| `write_ref_frames()` | `entropy_coding.c` | Encode reference frame selection (compound/single trees) |
| `write_mb_interp_filter()` | `entropy_coding.c` | Encode interpolation filter selection |
| `write_motion_mode()` | `entropy_coding.c` | Encode motion mode (simple/OBMC/warped) |

### Coefficient Coding

| Function | File | Purpose |
|----------|------|---------|
| `av1_write_coeffs_txb_1d()` | `entropy_coding.c` | Full coefficient coding for a transform block |
| `av1_write_tx_type()` | `entropy_coding.c` | Encode transform type |
| `av1_encode_tx_coef_y()` | `entropy_coding.c` | Encode luma TX coefficients for a block |
| `av1_encode_tx_coef_uv()` | `entropy_coding.c` | Encode chroma TX coefficients for a block |
| `av1_encode_coeff_1d()` | `entropy_coding.c` | Top-level coefficient encoding for all planes |
| `svt_aom_get_txb_ctx()` | `entropy_coding.c` | Derive TXB skip context and DC sign context from neighbors |
| `write_golomb()` | `entropy_coding.c` | Exponential-Golomb code for large coefficient residuals |
| `svt_aom_txb_estimate_coeff_bits()` | `entropy_coding.h` | Rate estimation for RDO (not actual coding) |

### Context Derivation

| Function | File | Purpose |
|----------|------|---------|
| `svt_aom_get_kf_y_mode_ctx()` | `entropy_coding.c` | KF Y mode context from above/left neighbor modes |
| `av1_get_skip_context()` | `entropy_coding.c` | Skip flag context from above/left |
| `av1_get_skip_mode_context()` | `entropy_coding.c` | Skip mode context from above/left |
| `svt_av1_get_intra_inter_context()` | `entropy_coding.c` | Intra/inter context from above/left (0-3) |
| `svt_aom_collect_neighbors_ref_counts_new()` | `entropy_coding.c` | Count reference frames in neighbors for ref frame contexts |
| `svt_av1_get_pred_context_single_ref_p1()` through `p6()` | `entropy_coding.h` | Single reference frame contexts |
| `svt_av1_get_pred_context_comp_ref_p()` etc. | `entropy_coding.h` | Compound reference frame contexts |
| `svt_aom_get_pred_context_switchable_interp()` | `entropy_coding.c` | Interpolation filter context from neighbors |
| `svt_aom_get_comp_index_context_enc()` | `entropy_coding.c` | Compound index context |
| `get_txsize_entropy_ctx()` | `entropy_coding.h` | TX size to entropy context mapping |

### Frame/Sequence Header

| Function | File | Purpose |
|----------|------|---------|
| `svt_aom_write_frame_header_av1()` | `entropy_coding.c` | Write complete frame header OBU |
| `write_uncompressed_header_obu()` | `entropy_coding.c` | Write all frame header fields |
| `svt_aom_encode_sps_av1()` | `entropy_coding.c` | Write sequence parameter set OBU |
| `svt_aom_encode_td_av1()` | `entropy_coding.c` | Write temporal delimiter OBU |
| `write_sequence_header_obu()` | `entropy_coding.c` | Write sequence header payload |
| `write_obu_header()` | `entropy_coding.c` | Write OBU header (type, extension, has_size) |
| `write_tile_group_header()` | `entropy_coding.c` | Write tile group header |
| `encode_quantization()` | `entropy_coding.c` | Write quantization parameters |
| `encode_segmentation()` | `entropy_coding.c` | Write segmentation parameters |
| `encode_loopfilter()` | `entropy_coding.c` | Write loop filter parameters |
| `encode_cdef()` (header) | `entropy_coding.c` | Write CDEF parameters to frame header |
| `encode_restoration_mode()` | `entropy_coding.c` | Write restoration mode to frame header |
| `write_tile_info()` | `entropy_coding.c` | Write tile configuration |
| `write_global_motion()` | `entropy_coding.c` | Write global motion parameters |
| `write_film_grain_params()` | `entropy_coding.c` | Write film grain parameters |

### CDF Management

| Function | File | Purpose |
|----------|------|---------|
| `update_cdf()` | `cabac_context_model.h` | Backward adaptation of CDF after coding a symbol |
| `svt_aom_init_mode_probs()` | `cabac_context_model.h` | Initialize CDFs to default values |
| `svt_av1_default_coef_probs()` | `cabac_context_model.h` | Initialize coefficient CDFs based on base QP |
| `svt_av1_reset_cdf_symbol_counters()` | `cabac_context_model.h` | Reset adaptation counters in all CDFs |
| `svt_aom_reset_entropy_coder()` | `entropy_coding.c` | Reset entropy coder with default CDFs for given QP/slice |

### Process-Level

| Function | File | Purpose |
|----------|------|---------|
| `svt_aom_entropy_coding_kernel()` | `ec_process.c` | Main thread loop: process tiles, encode SBs, signal completion |
| `svt_aom_entropy_coding_context_ctor()` | `ec_process.c` | Construct thread context |
| `svt_aom_encode_slice_finish()` | `entropy_coding.c` | Finalize a tile's arithmetic-coded bitstream |

## Dependencies

### Upstream Pipeline Inputs
- **Mode decision results**: Prediction modes, motion vectors, reference frames, transform types, partition decisions -- all stored in `MbModeInfo` and `EcBlkStruct`
- **Quantized coefficients**: `EbPictureBufferDesc` from the encode/decode stage
- **Loop restoration decisions**: `RestorationUnitInfo` per plane
- **CDEF strengths**: In `MbModeInfo::cdef_strength`
- **Frame header parameters**: `FrameHeader` structure (quantization, segmentation, loop filter, CDEF, global motion, film grain)

### Neighbor Arrays (Context Derivation)
- `partition_context_na` -- partition context for above/left
- `luma_dc_sign_level_coeff_na` -- DC sign and coefficient level for Y
- `cb_dc_sign_level_coeff_na` / `cr_dc_sign_level_coeff_na` -- for chroma
- `txfm_context_array` -- transform partition context
- `segmentation_id_pred_array` -- segment ID prediction
- `mi_grid_base` -- mode info grid for neighbor access

### Internal Dependencies
- `cabac_context_model.h/c` -- all CDF tables and `update_cdf()`
- `bitstream_unit.h/c` -- arithmetic coder and raw bit writer
- `ec_object.h` -- `EntropyCoder`, `Bitstream` structures
- `coding_unit.h` -- `EcBlkStruct`, block-level coding info
- `definitions.h` -- block size, TX size, mode enumerations
- `transforms.h` -- scan orders, TX size tables
- `adaptive_mv_pred.h` -- `svt_aom_mode_context_analyzer()`
- `rd_cost.c` -- rate estimation using the same CDFs (for mode decision)
- `full_loop.c` -- coefficient rate estimation during RDO

## SIMD Functions

The following entropy coding support functions have SIMD implementations:

| Function | Architectures | Purpose |
|----------|---------------|---------|
| `svt_av1_txb_init_levels` | SSE4.1, AVX2, AVX-512, NEON | Initialize the coefficient level buffer from quantized coefficients. Converts 32-bit coefficients to 8-bit absolute values with padding. |
| `svt_av1_get_nz_map_contexts` | SSE2, AVX2, NEON | Compute nonzero map contexts for all coefficient positions based on neighbor levels, scan order, and TX class. Used in coefficient coding step 5. |
| `svt_copy_mi_map_grid` | AVX2, NEON | Copy mode info pointer across a grid (fills MI grid for a block). Not directly entropy coding but used during context setup. |

The core arithmetic coder (`svt_od_ec_encode_*`, `svt_od_ec_enc_normalize`) has no SIMD implementations -- it is inherently serial due to range state dependencies. The `update_cdf()` function is also scalar (C inline), though the similar function used during rate estimation in mode decision (`md_rate_estimation.c`) does reference SIMD-optimized paths.

The SIMD functions for coefficient context computation (`svt_av1_txb_init_levels` and `svt_av1_get_nz_map_contexts`) are the primary performance-critical operations in entropy coding, as they are called for every non-zero transform block. Their implementations are in:

- `Source/Lib/ASM_SSE2/encodetxb_sse2.c`
- `Source/Lib/ASM_SSE4_1/encodetxb_sse4.c`
- `Source/Lib/ASM_AVX2/encodetxb_avx2.c`
- `Source/Lib/ASM_AVX512/encodetxb_avx512.c`
- `Source/Lib/ASM_NEON/encodetxb_neon.c`
- `Source/Lib/C_DEFAULT/encode_txb_ref_c.c` (reference C implementations)
