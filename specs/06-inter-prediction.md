# Inter Prediction

## Overview

Inter prediction generates prediction samples for a block by referencing previously coded frames. AV1 supports single-reference and compound (two-reference) modes, with sub-pixel interpolation, warped motion, overlapped block motion compensation (OBMC), and several compound blending strategies. SVT-AV1 implements the full AV1 inter prediction pipeline with both 8-bit and high-bit-depth paths.

The inter prediction pipeline:
1. Select reference frame(s) and motion vector(s)
2. Apply sub-pixel interpolation filters to generate prediction from reference
3. For compound modes: generate predictions from both references, then blend
4. For OBMC: blend with predictions from neighboring blocks
5. For warped motion: apply affine warp instead of translational motion

## Source Files

| File | Lines | Role |
|------|-------|------|
| `Source/Lib/Codec/inter_prediction.c` | 2581 | Core: scale factors, interpolation filter tables, convolve functions (2D, X-only, Y-only, copy, scaled), compound diff-weighted mask generation, wedge mask tables, interintra combine, distance-weighted compound weights |
| `Source/Lib/Codec/inter_prediction.h` | 551 | Data structures (SubpelParams, WedgeCodeType, WedgeParamsType), filter parameter tables, compound mode utilities, MV projection/clamping, reference frame type helpers |
| `Source/Lib/Codec/enc_inter_prediction.c` | 3883 | Encoder-side: top-level inter prediction dispatch, masked compound prediction, OBMC, inter-intra precompute, warped motion dispatch, model-based RD |
| `Source/Lib/Codec/enc_inter_prediction.h` | 94 | Encoder inter prediction API |
| `Source/Lib/Codec/convolve.c` | 310 | Wiener convolve (add-src variant for restoration filter), both LBD and HBD |
| `Source/Lib/Codec/convolve.h` | 106 | Convolve types, ConvolveParams initialization helpers, highbd convolve facade |
| `Source/Lib/Codec/blend_a64_mask.c` | 346 | Alpha-mask blending for compound modes: `lowbd_blend_a64_d16_mask`, `highbd_blend_a64_d16_mask` with subsampling support |
| `Source/Lib/Codec/firstpass.c` | 233 | First-pass analysis: stat accumulation, output, two-pass framework data flow |
| `Source/Lib/Codec/firstpass.h` | 111 | FIRSTPASS_STATS structure, TWO_PASS control data, STATS_BUFFER_CTX |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| `test/convolve_test.cc` | 2D separable convolution (all filter combinations, all block sizes) |
| `test/Convolve8Test.cc` | Legacy 8-tap convolve functions |
| `test/av1_convolve_scale_test.cc` | Scaled convolution (reference scaling) |
| `test/warp_filter_test.cc` | Warped motion affine prediction |
| `test/CompoundUtilTest.cc` | Compound utility functions (diff-weighted mask, distance weights) |
| `test/OBMCSadTest.cc` | OBMC SAD computation |
| `test/OBMCVarianceTest.cc` | OBMC variance computation |
| `test/WedgeUtilTest.cc` | Wedge mask generation and sign selection |
| `test/IntraBcUtilTest.cc` | IntraBC displacement vector validation (`svt_aom_is_dv_valid`) with parameterized test cases covering various block sizes, DV directions, boundary conditions, and tile constraints |

## Data Structures

### SubpelParams

```c
typedef struct SubpelParams {
    int32_t xs;         // horizontal step (SUBPEL_SHIFTS for no-scale)
    int32_t ys;         // vertical step
    int32_t subpel_x;   // horizontal sub-pixel offset (0..15 for q4)
    int32_t subpel_y;   // vertical sub-pixel offset
} SubpelParams;
```

### ConvolveParams

```c
typedef struct ConvolveParams {
    int           ref;
    int           do_average;       // 1 = blend with dst (compound second ref)
    int           is_compound;
    int           round_0;          // first-pass rounding bits
    int           round_1;          // second-pass rounding bits
    ConvBufType*  dst;              // intermediate compound buffer
    int           dst_stride;
    int           plane;
    int           use_jnt_comp_avg; // use distance-weighted averaging
    int           fwd_offset;       // forward reference weight
    int           bck_offset;       // backward reference weight
    int           use_dist_wtd_comp_avg;
} ConvolveParams;
```

### ScaleFactors

```c
typedef struct ScaleFactors {
    int x_scale_fp;    // horizontal scale in fixed-point (REF_SCALE_SHIFT=14 bits)
    int y_scale_fp;    // vertical scale
    int x_step_q4;     // horizontal step in q4
    int y_step_q4;     // vertical step in q4
    int (*scale_value_x)(int val, const ScaleFactors* sf);
    int (*scale_value_y)(int val, const ScaleFactors* sf);
} ScaleFactors;
```

`REF_NO_SCALE = 1 << 14 = 16384`. Scale is invalid when set to `REF_INVALID_SCALE = -1`.

### Interpolation Filter Types

Four switchable filters plus bilinear:
```c
EIGHTTAP_REGULAR  = 0  // sub_pel_filters_8 (8-tap, sharp)
EIGHTTAP_SMOOTH   = 1  // sub_pel_filters_8smooth
MULTITAP_SHARP    = 2  // sub_pel_filters_8sharp
BILINEAR          = 3  // bilinear_filters
```

For blocks <= 4 pixels wide, 4-tap variants are used instead of 8-tap:
- `sub_pel_filters_4` replaces EIGHTTAP_REGULAR and MULTITAP_SHARP
- `sub_pel_filters_4smooth` replaces EIGHTTAP_SMOOTH

Each filter bank has `SUBPEL_SHIFTS = 16` entries (including the identity at index 0). Each kernel is `SUBPEL_TAPS = 8` coefficients that sum to 128.

### WedgeCodeType / WedgeParamsType

```c
typedef struct WedgeCodeType {
    WedgeDirectionType direction;  // HORIZONTAL, VERTICAL, OBLIQUE27/63/117/153
    int32_t x_offset;
    int32_t y_offset;
} WedgeCodeType;

typedef struct WedgeParamsType {
    int32_t              bits;      // number of bits to signal wedge index
    const WedgeCodeType* codebook;
    uint8_t*             signflip;
    WedgeMasksType*      masks;     // precomputed wedge masks
} WedgeParamsType;
```

### Compound Types

```c
COMPOUND_AVERAGE  = 0  // simple average (or distance-weighted)
COMPOUND_DISTWTD  = 1  // distance-weighted compound
COMPOUND_DIFFWTD  = 2  // difference-weighted compound (adaptive mask)
COMPOUND_WEDGE    = 3  // wedge-shaped mask
```

### FIRSTPASS_STATS

```c
typedef struct {
    double     frame;        // frame number
    double     coded_error;  // best of intra/inter error
    double     duration;
    double     count;        // 1.0 for single frame
    StatStruct stat_struct;  // POC, qindex, total_bits, temporal_layer, worst_qindex
} FIRSTPASS_STATS;
```

## Algorithms

### 1. Sub-Pixel Interpolation (Single Reference)

The core interpolation is a 2D separable convolution:

#### Horizontal-only (`svt_av1_convolve_x_sr_c`)
When `subpel_y == 0`:
```
x_filter = filter_kernel[subpel_x & SUBPEL_MASK]   // 8-tap kernel
for each pixel (r, c):
    res = sum(x_filter[k] * src[r][c - fo_horiz + k] for k in 0..taps-1)
    dst[r][c] = clip(round(round(res, round_0), bits))
where fo_horiz = taps/2 - 1, bits = FILTER_BITS - round_0
```

#### Vertical-only (`svt_av1_convolve_y_sr_c`)
When `subpel_x == 0`:
```
y_filter = filter_kernel[subpel_y & SUBPEL_MASK]
for each pixel (r, c):
    res = sum(y_filter[k] * src[r - fo_vert + k][c] for k in 0..taps-1)
    dst[r][c] = clip(round(res, FILTER_BITS))
```

#### 2D Separable (`svt_av1_convolve_2d_sr_c`)
Two-pass filtering:
```
// Pass 1: Horizontal into intermediate buffer (16-bit)
for each pixel in extended rows:
    sum = (1 << (bd + FILTER_BITS - 1))  // bias
    sum += sum(x_filter[k] * src_horiz[k] for k in 0..taps-1)
    im_block[y][x] = round(sum, round_0)

// Pass 2: Vertical from intermediate to output
for each output pixel:
    sum = 1 << offset_bits
    sum += sum(y_filter[k] * im_block[y-fo+k][x] for k in 0..taps-1)
    res = round(sum, round_1) - round_offset
    dst[y][x] = clip(round(res, bits))
```

Where:
- `round_0 = ROUND0_BITS = 3`
- `round_1 = 2 * FILTER_BITS - round_0` (for single ref) or `COMPOUND_ROUND1_BITS = 7` (for compound)
- `FILTER_BITS = 7`
- `offset_bits = bd + 2 * FILTER_BITS - round_0`
- `bits = 2 * FILTER_BITS - round_0 - round_1`

#### Copy (`svt_av1_convolve_2d_copy_sr_c`)
When both `subpel_x == 0` and `subpel_y == 0`: direct pixel copy.

### 2. Scaled Convolution

`svt_av1_convolve_2d_scale_c` handles reference scaling (when reference frame has different dimensions):

```
// Horizontal pass with variable step
for each row:
    x_qn = subpel_x_qn
    for each output column:
        src_x = src_horiz[x_qn >> SCALE_SUBPEL_BITS]
        x_filter_idx = (x_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS
        x_filter = kernel[x_filter_idx]
        // apply filter...
        x_qn += x_step_qn

// Vertical pass with variable step
for each column:
    y_qn = subpel_y_qn
    for each output row:
        src_y = src_vert[(y_qn >> SCALE_SUBPEL_BITS) * im_stride]
        y_filter_idx = (y_qn & SCALE_SUBPEL_MASK) >> SCALE_EXTRA_BITS
        // apply filter...
        y_qn += y_step_qn
```

Where `SCALE_SUBPEL_BITS = 10`, `SCALE_EXTRA_BITS = SCALE_SUBPEL_BITS - SUBPEL_BITS = 6`.

### 3. Compound Prediction Modes

When two references are used, both predictions are generated and combined.

#### Compound Average / Distance-Weighted (`svt_av1_jnt_convolve_*`)

For compound modes, the first reference is stored in an intermediate buffer (`conv_params->dst`). The second reference is convolved and combined:

```
// Second reference result:
res = round(sum, round_1)

if do_average:
    tmp = dst[previous_ref]
    if use_jnt_comp_avg:
        tmp = (tmp * fwd_offset + res * bck_offset) >> DIST_PRECISION_BITS
    else:
        tmp = (tmp + res) >> 1
    tmp -= round_offset
    output = clip(round(tmp, round_bits))
else:
    dst[y][x] = res   // store for later blending
```

`DIST_PRECISION_BITS = 4`.

#### Distance-Weighted Compound Weight Assignment

`svt_av1_dist_wtd_comp_weight_assign`:

1. Compute temporal distances: `d0 = |fwd_frame - cur_frame|`, `d1 = |cur_frame - bck_frame|`
2. Use quantized distance lookup tables to determine `fwd_offset` and `bck_offset` (sum to 16)
3. Closer references get higher weight

The lookup table provides 4 quantization levels:
```
quant_dist_weight[4][2] = {{2,3}, {2,5}, {2,7}, {1, MAX_FRAME_DISTANCE}}
quant_dist_lookup_table[2][4][2] = {
    {{9,7}, {11,5}, {12,4}, {13,3}},
    {{7,9}, {5,11}, {4,12}, {3,13}}
}
```

#### Difference-Weighted Compound (DIFFWTD)

`svt_av1_build_compound_diffwtd_mask_c` generates a per-pixel mask based on the absolute difference between two predictions:

```
for each pixel (i, j):
    diff = |src0[i][j] - src1[i][j]|
    m = clamp(mask_base + diff / DIFF_FACTOR, 0, AOM_BLEND_A64_MAX_ALPHA)
    mask[i][j] = which_inverse ? (64 - m) : m
```

Where `mask_base = 38`, `DIFF_FACTOR` is a constant, and `AOM_BLEND_A64_MAX_ALPHA = 64`.

Two mask types: `DIFFWTD_38` (normal) and `DIFFWTD_38_INV` (inverted).

For HBD with `bd > 8`, the difference is right-shifted by `bd - 8` before dividing by DIFF_FACTOR.

#### Wedge Compound

Uses precomputed wedge-shaped masks stored in `wedge_masks[BLOCK_SIZES_ALL][2]`. The mask is selected by `wedge_index` and `wedge_sign`:

```
mask = svt_aom_get_contiguous_soft_mask(wedge_index, wedge_sign, bsize)
```

Wedge directions include horizontal, vertical, and four oblique angles (27, 63, 117, 153 degrees). The number of wedge types varies by block size (obtained via `svt_aom_get_wedge_params_bits`).

#### Masked Compound Blending

`svt_aom_build_masked_compound_no_round`: Applies the compound mask (from DIFFWTD or wedge) to blend two predictions. Dispatches to `svt_aom_lowbd_blend_a64_d16_mask_c` or `svt_aom_highbd_blend_a64_d16_mask_c`.

### 4. Alpha Mask Blending

`svt_aom_lowbd_blend_a64_d16_mask_c` / `svt_aom_highbd_blend_a64_d16_mask_c`:

For the d16 (intermediate precision) variant:
```
round_offset = (1 << (offset_bits - round_1)) + (1 << (offset_bits - round_1 - 1))

for each pixel:
    m = mask[i][j]  // possibly subsampled for chroma
    res = (m * src0[i][j] + (64 - m) * src1[i][j]) >> 6
    res -= round_offset
    output = clip(round(res, round_bits))
```

Mask subsampling for chroma:
- `subw=0, subh=0`: use mask directly
- `subw=1, subh=1`: average 2x2 mask samples
- `subw=1, subh=0`: average horizontal pair
- `subw=0, subh=1`: average vertical pair

### 5. Inter-Intra Compound

`svt_aom_combine_interintra` blends an inter prediction with an intra prediction:

Four inter-intra modes map to intra modes: `{DC_PRED, V_PRED, H_PRED, SMOOTH_PRED}`.

**Without wedge**: Uses smooth inter-intra masks (`get_ii_mask`) that vary by block size and mode. Blended via `svt_aom_blend_a64_mask`.

**With wedge**: Uses the same wedge masks as inter-inter compound, but always with `INTERINTRA_WEDGE_SIGN = 0`. Limited to block sizes where `svt_aom_is_interintra_wedge_used(bsize)` returns true, and max block area is `32 * 32`.

### 6. OBMC (Overlapped Block Motion Compensation)

OBMC blends the current block's prediction with predictions generated using motion vectors from overlapping neighboring blocks.

#### OBMC Mask

`svt_av1_get_obmc_mask(length)` returns a 1D blending mask of the given length. The mask values ramp from high (near the block edge) to low (toward the interior), ensuring smooth transitions.

#### OBMC Process

1. **Above neighbors**: For each overlappable above neighbor:
   - Generate prediction using the neighbor's MV into a temporary buffer
   - Blend vertically: `svt_aom_blend_a64_vmask(dst, dst, tmp, obmc_mask, bw, bh)`

2. **Left neighbors**: For each overlappable left neighbor:
   - Generate prediction using the neighbor's MV into a temporary buffer
   - Blend horizontally: `svt_aom_blend_a64_hmask(dst, dst, tmp, obmc_mask, bw, bh)`

OBMC requires `block_size >= 8x8` (`is_motion_variation_allowed_bsize`).

The `DISABLE_CHROMA_U8X8_OBMC` flag controls whether chroma OBMC is disabled for small blocks (< 8x8 in chroma space).

### 7. Warped Motion Prediction

`svt_av1_warp_affine_c` applies an affine transformation to the reference frame:

Parameters: 6-element affine matrix `mat[6]`, plus derived shear parameters `alpha, beta, gamma, delta`.

The warped prediction is applied when:
- `is_global_mv_block` returns true (global motion with non-translational type)
- Local warp parameters are available for the block
- Block size >= 8x8

The warp applies the affine transform per-pixel, using 8-tap interpolation filters for sub-pixel accuracy.

### 8. Interpolation Filter Selection

`av1_get_interp_filter_params_with_block_size`: Selects filter parameters based on filter type and block width:

```
if w <= 4 and (filter == MULTITAP_SHARP or filter == EIGHTTAP_REGULAR):
    return av1_interp_4tap[0]    // 4-tap regular
elif w <= 4 and filter == EIGHTTAP_SMOOTH:
    return av1_interp_4tap[1]    // 4-tap smooth
else:
    return av1_interp_filter_params_list[filter]  // standard 8-tap
```

`av1_get_convolve_filter_params`: Extracts X and Y filters from the combined `InterpFilters` value (two 4-bit filter indices packed into a uint32_t).

### 9. Scale Factor Setup

`svt_av1_setup_scale_factors_for_frame`:

```
sf->x_scale_fp = (other_w << REF_SCALE_SHIFT + other_w/2) / this_w
sf->y_scale_fp = (other_h << REF_SCALE_SHIFT + other_h/2) / this_h
sf->x_step_q4 = round(x_scale_fp, REF_SCALE_SHIFT - SCALE_SUBPEL_BITS)
sf->y_step_q4 = round(y_scale_fp, REF_SCALE_SHIFT - SCALE_SUBPEL_BITS)
```

Valid scaling range: reference must be between 1/2x and 16x of current frame dimensions.

### 10. MV Precision and Projection

**Integer MV precision** (`integer_mv_precision`): Rounds MV to nearest 8th-pel boundary (full pixel in 1/8-pel units).

**Lower MV precision** (`lower_mv_precision`): When quarter-pel only (`!allow_hp`), rounds away 1/8-pel component.

**MV projection** (`get_mv_projection`): Projects a reference MV to a different temporal distance:
```
mv_row = round(ref.y * num * div_mult[den], 14)
mv_col = round(ref.x * num * div_mult[den], 14)
```

Where `div_mult[32]` is a precomputed table of `16384 / i` for `i = 0..31`.

### 11. Wiener Convolution

`svt_av1_wiener_convolve_add_src_c` / `svt_av1_highbd_wiener_convolve_add_src_c`: Used by the Wiener restoration filter (not standard inter prediction).

Two-pass separable filtering with an add-source bias:
```
// Horizontal pass:
rounding = src_center << FILTER_BITS + (1 << (bd + FILTER_BITS - 1))
sum = horz_scalar_product(src, x_filter) + rounding
im[y][x] = clamp(round(sum, round_0), 0, WIENER_CLAMP_LIMIT - 1)

// Vertical pass:
rounding = src_center << FILTER_BITS - (1 << (bd + round_1 - 1))
sum = vert_scalar_product(src, y_filter) + rounding
dst[y][x] = clip(round(sum, round_1))
```

The `WIENER_CLAMP_LIMIT(r0, bd) = 1 << (bd + 1 + FILTER_BITS - r0)`.

### 12. First-Pass Analysis

The first pass collects frame-level statistics for rate control:

**FIRSTPASS_STATS** accumulates:
- Frame number, coded error (best of intra/inter prediction error)
- Duration, count
- Extended statistics via `StatStruct` (POC, QP index, total bits, temporal layer, worst QP)

**Key functions:**
- `output_stats`: Write stats to the output buffer (thread-safe via mutex)
- `svt_av1_twopass_zero_stats`: Initialize a stats structure
- `svt_av1_accumulate_stats`: Accumulate frame stats into a running total
- `update_firstpass_stats`: Store per-frame stats and update the two-pass buffer
- `svt_av1_end_first_pass`: Finalize first pass by writing total stats

**TWO_PASS** control structure manages:
- Stats buffer with circular or linear mode (depending on LAP/single-pass)
- Remaining bits budget (`bits_left`)
- KF group bit allocation (`kf_group_bits`, `kf_group_error_left`)
- QP extension parameters (`extend_minq`, `extend_maxq`)

## Key Functions

| Function | File | Purpose |
|----------|------|---------|
| `svt_aom_inter_prediction` | enc_inter_prediction.c | Top-level inter prediction: handles all modes, planes, compound, OBMC, inter-intra |
| `svt_aom_enc_make_inter_predictor` | enc_inter_prediction.c | Single-plane inter prediction with optional compound mask |
| `svt_inter_predictor` | inter_prediction.h (inline)/c | LBD inter prediction dispatch based on subpel/scale |
| `svt_highbd_inter_predictor` | inter_prediction.h/c | HBD inter prediction dispatch |
| `svt_inter_predictor_light_pd0` | inter_prediction.h | Simplified predictor for PD0 (no filters, no scale) |
| `svt_inter_predictor_light_pd1` | inter_prediction.h | Simplified predictor for PD1 |
| `svt_av1_convolve_2d_sr_c` | inter_prediction.c | 2D separable convolution, single reference |
| `svt_av1_convolve_x_sr_c` | inter_prediction.c | Horizontal-only convolution |
| `svt_av1_convolve_y_sr_c` | inter_prediction.c | Vertical-only convolution |
| `svt_av1_convolve_2d_copy_sr_c` | inter_prediction.c | Full-pixel copy |
| `svt_av1_convolve_2d_scale_c` | inter_prediction.c | Scaled 2D convolution |
| `svt_av1_jnt_convolve_2d_c` | inter_prediction.c | 2D compound (joint) convolution |
| `svt_av1_jnt_convolve_x_c` | inter_prediction.c | Horizontal-only compound convolution |
| `svt_av1_jnt_convolve_y_c` | inter_prediction.c | Vertical-only compound convolution |
| `svt_av1_jnt_convolve_2d_copy_c` | inter_prediction.c | Full-pixel compound copy |
| `svt_av1_dist_wtd_comp_weight_assign` | inter_prediction.c | Distance-weighted compound weight calculation |
| `svt_av1_build_compound_diffwtd_mask_c` | inter_prediction.c | Diff-weighted compound mask generation (LBD) |
| `svt_av1_build_compound_diffwtd_mask_highbd_c` | inter_prediction.c | Diff-weighted compound mask generation (HBD) |
| `svt_aom_build_masked_compound_no_round` | inter_prediction.h | Apply compound mask to blend two predictions |
| `svt_aom_lowbd_blend_a64_d16_mask_c` | blend_a64_mask.c | Alpha-mask blending in intermediate precision (LBD) |
| `svt_aom_highbd_blend_a64_d16_mask_c` | blend_a64_mask.c | Alpha-mask blending in intermediate precision (HBD) |
| `svt_aom_combine_interintra` | inter_prediction.c | Inter-intra blending (LBD) |
| `svt_aom_combine_interintra_highbd` | inter_prediction.c | Inter-intra blending (HBD) |
| `svt_aom_precompute_obmc_data` | enc_inter_prediction.c | Precompute OBMC predictions from neighbors |
| `svt_av1_get_obmc_mask` | enc_inter_prediction.c | Get OBMC blending mask for given overlap length |
| `svt_aom_get_contiguous_soft_mask` | inter_prediction.c | Get wedge mask for given index/sign/bsize |
| `svt_av1_setup_scale_factors_for_frame` | inter_prediction.c | Initialize scale factors for reference scaling |
| `svt_av1_warp_affine_c` | warped_motion.c | Affine warp prediction |
| `svt_aom_search_compound_diff_wedge` | enc_inter_prediction.c | Search best diff-weighted or wedge compound params |
| `svt_aom_calc_pred_masked_compound` | enc_inter_prediction.c | Compute masked compound prediction |
| `svt_aom_find_ref_dv` | inter_prediction.c | Find reference displacement vector for IntraBC |
| `model_rd_from_sse` | enc_inter_prediction.c | RD model from SSE for compound mode decisions |
| `svt_av1_wiener_convolve_add_src_c` | convolve.c | Wiener restoration convolve (LBD) |
| `svt_av1_highbd_wiener_convolve_add_src_c` | convolve.c | Wiener restoration convolve (HBD) |
| `update_firstpass_stats` | firstpass.c | Store first-pass per-frame statistics |
| `svt_av1_end_first_pass` | firstpass.c | Finalize first-pass stats |

## Dependencies

- **Block structures**: `BlockSize`, `BlockModeInfo`, `BlkStruct`, `PredictionMode` from `definitions.h`, `block_structures.h`
- **Motion vectors**: `Mv` struct, MV clamping/projection utilities
- **Reference frames**: `MvReferenceFrame` enum (LAST through ALTREF), `EbPictureBufferDesc` for reference picture buffers
- **Filter definitions**: `InterpFilterParams`, `InterpKernel` from `filter.h`
- **Mode decision**: Called from `mode_decision.c` and `coding_loop.c`
- **Warped motion**: `warped_motion.c` / `warped_motion.h` for affine warp computation
- **Intra prediction**: Inter-intra compound requires intra prediction (`enc_intra_prediction.h`)
- **Rate control**: First-pass feeds into `TWO_PASS` for second-pass rate allocation

## SIMD Functions

| C Function | SIMD Variants | Notes |
|------------|---------------|-------|
| `svt_av1_convolve_2d_sr_c` | SSE2, AVX2, NEON | 2D separable convolution |
| `svt_av1_convolve_x_sr_c` | SSE2, AVX2, NEON | Horizontal convolution |
| `svt_av1_convolve_y_sr_c` | SSE2, AVX2, NEON | Vertical convolution |
| `svt_av1_convolve_2d_copy_sr_c` | SSE2, AVX2, NEON | Pixel copy |
| `svt_av1_convolve_2d_scale_c` | SSE4.1, NEON | Scaled 2D convolution |
| `svt_av1_jnt_convolve_2d_c` | SSE2, AVX2, NEON | Compound 2D convolution |
| `svt_av1_jnt_convolve_x_c` | SSE2, AVX2, NEON | Compound horizontal convolution |
| `svt_av1_jnt_convolve_y_c` | SSE2, AVX2, NEON | Compound vertical convolution |
| `svt_av1_jnt_convolve_2d_copy_c` | SSE2, AVX2, NEON | Compound pixel copy |
| `svt_av1_highbd_convolve_2d_sr_c` | SSE4.1, AVX2, NEON | HBD 2D convolution |
| `svt_av1_highbd_convolve_x_sr_c` | SSE4.1, AVX2, NEON | HBD horizontal convolution |
| `svt_av1_highbd_convolve_y_sr_c` | SSE4.1, AVX2, NEON | HBD vertical convolution |
| `svt_av1_highbd_convolve_2d_copy_sr_c` | SSE4.1, AVX2, NEON | HBD pixel copy |
| `svt_av1_highbd_convolve_2d_scale_c` | SSE4.1, NEON | HBD scaled convolution |
| `svt_av1_highbd_jnt_convolve_2d_c` | SSE4.1, AVX2, NEON | HBD compound 2D |
| `svt_av1_highbd_jnt_convolve_x_c` | SSE4.1, AVX2, NEON | HBD compound horizontal |
| `svt_av1_highbd_jnt_convolve_y_c` | SSE4.1, AVX2, NEON | HBD compound vertical |
| `svt_av1_highbd_jnt_convolve_2d_copy_c` | SSE4.1, AVX2, NEON | HBD compound copy |
| `svt_av1_warp_affine_c` | SSE4.1, AVX2, NEON, NEON-I8MM, SVE | Affine warp |
| `svt_av1_highbd_warp_affine_c` | SSE4.1, AVX2, NEON | HBD affine warp |
| `svt_av1_build_compound_diffwtd_mask_c` | SSE4.1, AVX2, NEON | Diff-weighted mask |
| `svt_av1_build_compound_diffwtd_mask_highbd_c` | SSE4.1, AVX2, NEON | HBD diff-weighted mask |
| `svt_aom_lowbd_blend_a64_d16_mask_c` | SSE4.1, AVX2, NEON | Alpha blending (LBD) |
| `svt_aom_highbd_blend_a64_d16_mask_c` | SSE4.1, AVX2, NEON | Alpha blending (HBD) |
| `svt_aom_blend_a64_mask_c` | SSE4.1, AVX2, NEON | Non-d16 alpha blending |
| `svt_aom_blend_a64_vmask_c` | SSE4.1, NEON | Vertical mask blending (OBMC) |
| `svt_aom_blend_a64_hmask_c` | SSE4.1, NEON | Horizontal mask blending (OBMC) |
| `svt_av1_wiener_convolve_add_src_c` | SSE2, AVX2, NEON | Wiener convolve |
| `svt_av1_highbd_wiener_convolve_add_src_c` | SSE4.1, AVX2, NEON | HBD Wiener convolve |
