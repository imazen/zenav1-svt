# Intra Prediction

## Overview

Intra prediction generates prediction samples for a block using only already-reconstructed samples from the current frame. AV1 provides 13 angular/non-angular intra modes, plus special modes: Chroma-from-Luma (CfL), palette, and filter-intra. SVT-AV1 implements all spec-compliant modes with both 8-bit (LBD) and high-bit-depth (HBD, 10/12-bit) paths.

The prediction pipeline:
1. Gather reference samples from above and left neighbor arrays
2. Pad/extend unavailable reference samples
3. Optionally filter reference edges (directional modes only)
4. Optionally upsample reference edges (small blocks, shallow angles)
5. Generate prediction using the selected mode's algorithm
6. For CfL: add scaled luma AC component to DC chroma prediction

## Source Files

| File | Lines | Role |
|------|-------|------|
| `Source/Lib/Codec/intra_prediction.c` | 2674 | Core prediction kernels: DC, V, H, smooth, paeth, directional (z1/z2/z3), CfL subsampling/subtract-average, filter-intra, edge filtering, reference availability tables |
| `Source/Lib/Codec/intra_prediction.h` | 221 | Function pointer types, CfL macros, mode-to-angle map, edge requirement flags, utility inlines |
| `Source/Lib/Codec/enc_intra_prediction.c` | 902 | Encoder-side: `build_intra_predictors` (reference assembly + edge filtering + dispatch), `svt_av1_predict_intra_block` (top-level entry), neighbor array population |
| `Source/Lib/Codec/enc_intra_prediction.h` | 68 | Encoder intra prediction API declarations |
| `Source/Lib/Codec/palette.c` | 987 | Palette mode: k-means clustering, color cache, palette search for luma and chroma |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| `test/intrapred_test.cc` | All non-directional intra predictor functions (DC, V, H, smooth variants, paeth) for all TX sizes, LBD and HBD |
| `test/intrapred_dr_test.cc` | Directional prediction (z1, z2, z3) for all angles and block sizes |
| `test/intrapred_edge_filter_test.cc` | `svt_av1_filter_intra_edge` and `svt_av1_filter_intra_edge_high` edge filtering |
| `test/intrapred_cfl_test.cc` | CfL luma subsampling (420) and subtract-average for all valid CfL TX sizes |
| `test/FilterIntraPredTest.cc` | Filter-intra predictor for all 5 filter-intra modes and TX sizes |
| `test/highbd_intra_prediction_tests.cc` | High-bit-depth variants of all intra predictors |
| `test/PaletteModeUtilTest.cc` | Palette utility functions: `svt_av1_count_colors` / `_highbd` (LBD and 8/10/12-bit HBD), k-means clustering (`av1_k_means_dim1`, `av1_k_means_dim2`) correctness and SIMD (AVX2, NEON) index calculation (`svt_av1_calc_indices_dim1/2`) |
| `test/subtract_avg_cfl_test.cc` | CfL subtract-average SIMD validation: compares `subtract_average` AVX2/NEON output against C reference for all valid CfL TX sizes (4x4 through 32x32) |

## Data Structures

### Function Pointer Types

```
IntraPredFnC(dst, stride, w, h, above, left)           // Size-generic, LBD
IntraHighBdPredFnC(dst, stride, w, h, above, left, bd) // Size-generic, HBD
IntraPredFn(dst, stride, above, left)                   // Size-specific, LBD
IntraHighPredFn(dst, stride, above, left, bd)           // Size-specific, HBD
```

### Dispatch Tables

- `svt_aom_eb_pred[INTRA_MODES][TX_SIZES_ALL]` -- non-DC mode predictors (LBD)
- `svt_aom_dc_pred[2][2][TX_SIZES_ALL]` -- DC predictors indexed by `[have_left][have_above]`
- `svt_aom_pred_high[INTRA_MODES][TX_SIZES_ALL]` -- HBD non-DC predictors
- `svt_aom_dc_pred_high[2][2][TX_SIZES_ALL]` -- HBD DC predictors

### Reference Requirement Flags

```
NEED_LEFT       = 1 << 1
NEED_ABOVE      = 1 << 2
NEED_ABOVERIGHT = 1 << 3
NEED_ABOVELEFT  = 1 << 4
NEED_BOTTOMLEFT = 1 << 5
```

The `extend_modes[INTRA_MODES]` array maps each mode to its required reference flags.

### Mode-to-Angle Map

```
mode_to_angle_map[] = { 0, 90, 180, 45, 135, 113, 157, 203, 67, 0, 0, 0, 0 }
```

Indices correspond to prediction modes: DC=0, V=1(90deg), H=2(180deg), D45=3, D135=4, D113=5, D157=6, D203=7, D67=8, SMOOTH=9, SMOOTH_V=10, SMOOTH_H=11, PAETH=12. The actual angle for directional modes is `mode_to_angle_map[mode] + angle_delta * ANGLE_STEP` where `ANGLE_STEP = 3` degrees.

## Algorithms

### 1. DC Prediction

Four variants based on reference sample availability:

**DC (both available):** Average all `bw` above samples and `bh` left samples:
```
expected_dc = (sum_above + sum_left + (count >> 1)) / count
where count = bw + bh
```
Fill the entire block with `expected_dc`.

**DC Top (only above available):** Average the `bw` above samples; fill block.

**DC Left (only left available):** Average the `bh` left samples; fill block.

**DC 128 (neither available):** Fill with `128` (LBD) or `128 << (bd - 8)` (HBD).

### 2. Vertical Prediction (V_PRED)

Copy the above row to every row of the prediction block:
```
for r in 0..bh:
    dst[r] = copy of above[0..bw]
```

### 3. Horizontal Prediction (H_PRED)

Fill each row with the corresponding left sample:
```
for r in 0..bh:
    memset(dst[r], left[r], bw)
```

### 4. Smooth Prediction

Uses quadratic weight arrays (`sm_weight_arrays`) indexed by block dimension. Weights decrease from 255 (at edge) toward a small value (at opposite edge), scaled by `2^sm_weight_log2_scale` where `sm_weight_log2_scale = 8`.

**SMOOTH_PRED (bi-directional):**
```
below_pred = left[bh - 1]    // bottom-left pixel estimate
right_pred = above[bw - 1]   // top-right pixel estimate
sm_weights_w = sm_weight_arrays + bw
sm_weights_h = sm_weight_arrays + bh

for each pixel (r, c):
    pred = sm_weights_h[r] * above[c]
         + (scale - sm_weights_h[r]) * below_pred
         + sm_weights_w[c] * left[r]
         + (scale - sm_weights_w[c]) * right_pred
    dst[r][c] = pred >> (1 + sm_weight_log2_scale)
```

**SMOOTH_V_PRED (vertical only):** Blends `above[c]` and `below_pred` using `sm_weights_h[r]`.

**SMOOTH_H_PRED (horizontal only):** Blends `left[r]` and `right_pred` using `sm_weights_w[c]`.

### 5. Paeth Prediction

For each pixel, selects the reference sample (left, above, or above-left) whose value is closest to `base = top + left - top_left`:

```
top_left = above[-1]
for each pixel (r, c):
    base = above[c] + left[r] - top_left
    p_left     = |base - left[r]|
    p_top      = |base - above[c]|
    p_top_left = |base - top_left|
    if p_left <= p_top and p_left <= p_top_left:
        dst[r][c] = left[r]
    elif p_top <= p_top_left:
        dst[r][c] = above[c]
    else:
        dst[r][c] = top_left
```

### 6. Directional Prediction (All Angular Modes)

Directional modes predict along a line at a given angle. The angle range is 0-270 degrees (exclusive), split into three zones:

**Angle derivatives:** The `eb_dr_intra_derivative[90]` table provides fixed-point (scaled by 256) slope values for each 3-degree angle increment.

- `get_dx(angle)`: X-shift per unit Y-change. Used for angles 0-180.
- `get_dy(angle)`: Y-shift per unit X-change. Used for angles 90-270.

#### Zone 1: 0 < angle < 90 (above reference only)

Predicts along a line going up-right. For each row `r`, compute fractional position along above array:
```
x = r * dx
base = x >> frac_bits      // integer index into above[]
shift = ((x << upsample_above) & 0x3F) >> 1   // fractional part, 0..31

val = above[base] * (32 - shift) + above[base + 1] * shift
dst[r][c] = round(val, 5)
```
When base exceeds `max_base_x = (bw + bh - 1) << upsample_above`, clamp to `above[max_base_x]`.

#### Zone 2: 90 < angle < 180 (above and left references)

A transitional zone. For each pixel (r, c), determine whether the projection hits the above row or left column:
```
base_x = (row-dependent offset) >> frac_bits_x
if base_x >= min_base_x:
    interpolate from above[]
else:
    interpolate from left[]
```

#### Zone 3: 180 < angle < 270 (left reference only)

Mirror of Zone 1 but using the left column. For each column `c`:
```
y = c * dy
base = y >> frac_bits
shift = ((y << upsample_left) & 0x3F) >> 1

val = left[base] * (32 - shift) + left[base + 1] * shift
dst[r][c] = round(val, 5)
```

#### Exact 90/180 Degrees

Angle 90 dispatches to V_PRED; angle 180 dispatches to H_PRED.

### 7. Edge Filtering for Directional Modes

Before directional prediction, reference samples may be filtered and/or upsampled.

#### Edge Filter

`svt_av1_filter_intra_edge(p, sz, strength)` applies a 5-tap filter to the reference array:

```
Strength 1: kernel = {0, 4, 8, 4, 0}  (sum=16)
Strength 2: kernel = {0, 5, 6, 5, 0}  (sum=16)
Strength 3: kernel = {2, 4, 4, 4, 2}  (sum=16)
```

For each sample `i` in `1..sz-1`:
```
s = sum(edge[clamp(i-2+j, 0, sz-1)] * kernel[j] for j in 0..4)
p[i] = (s + 8) >> 4
```

#### Edge Filter Strength Selection

`svt_aom_intra_edge_filter_strength(bs0, bs1, delta, type)` determines strength (0-3) based on:
- `blk_wh = bs0 + bs1` (sum of block dimensions)
- `d = |delta|` (angle offset from cardinal)
- `type`: 0 = normal, 1 = smooth neighbor (filter type based on whether adjacent block uses smooth mode)

Larger blocks and steeper angles get stronger filtering.

#### Edge Corner Filter

When both above and left references are needed and the combined block size is >= 24:
```
s = left[0] * 5 + above[-1] * 6 + above[0] * 5
above[-1] = left[-1] = (s + 8) >> 4
```

#### Edge Upsample

`svt_aom_use_intra_edge_upsample(bs0, bs1, delta, type)` returns 1 if:
- `|delta|` is in range (1..39)
- Block is small enough: type=0 requires `blk_wh <= 16`, type=1 requires `blk_wh <= 8`

When upsampled, each reference sample is expanded to two samples (doubles resolution), and the directional predictor uses `frac_bits = 5` instead of `6`, with `base_inc = 2`.

### 8. Chroma-from-Luma (CfL) Algorithm

CfL predicts chroma samples as a DC prediction plus a scaled version of the reconstructed luma AC component.

#### Step 1: Luma Subsampling (4:2:0)

`svt_cfl_luma_subsampling_420_lbd_c` / `_hbd_c`: Average a 2x2 luma block into one chroma-resolution sample:
```
for j in 0..height step 2:
    for i in 0..width step 2:
        output_q3[i/2] = (input[i] + input[i+1] + input[i+stride] + input[i+stride+1]) << 1
```

The output is in Q3 format (scaled by 8, since `<< 1` on a sum of 4 gives effective `<< 3` relative to average).

#### Step 2: Subtract Average

`svt_subtract_average_c`: Compute and subtract the DC component:
```
sum_q3 = sum of all pred_buf_q3 samples
avg_q3 = (sum_q3 + round_offset) >> num_pel_log2
pred_buf_q3[i] -= avg_q3   // for all samples
```

This leaves only the AC (detail) component.

#### Step 3: Predict Chroma

`svt_cfl_predict_lbd_c` / `_hbd_c`: Generate the final chroma prediction:
```
for each pixel (r, c):
    scaled_luma = (pred_buf_q3[r * CFL_BUF_LINE + c] * alpha_q3 + 32) >> 6
    dst[r][c] = clip(pred[r][c] + scaled_luma, 0, max_val)
```

Where:
- `pred` = DC chroma prediction
- `alpha_q3` = scaling factor in Q3 (range -16..16 excluding 0)
- `CFL_BUF_LINE = 32` (stride of the CfL buffer)

#### Alpha Derivation

```
cfl_idx_to_alpha(alpha_idx, joint_sign, pred_type):
    alpha_sign = CFL_SIGN_U(joint_sign) or CFL_SIGN_V(joint_sign)
    if alpha_sign == CFL_SIGN_ZERO: return 0
    abs_alpha_q3 = CFL_IDX_U(alpha_idx) or CFL_IDX_V(alpha_idx)
    return (alpha_sign == CFL_SIGN_POS) ? abs_alpha_q3 + 1 : -abs_alpha_q3 - 1
```

CfL is valid for TX sizes up to 32x32 (no 64x64).

### 9. Palette Mode

Palette mode represents a block using a small set of colors (2-8) and a per-pixel color index map.

#### Palette Derivation (Encoder)

1. **Count colors** in the source block (`svt_av1_count_colors` / `_highbd`)
2. If colors <= 1 or > 64, skip palette
3. **Extract dominant colors** from the count histogram (top-N most frequent)
4. **K-means clustering**: `av1_k_means(data, centroids, indices, n, k, dim, max_itr)` iteratively refines centroid positions
5. **Optimize with color cache**: `optimize_palette_colors` biases centroids toward colors found in the palette cache (from above/left neighbors) when within a threshold
6. **Remove duplicates**: `av1_remove_duplicates` sorts and deduplicates centroids
7. **Assign indices**: `av1_calc_indices` maps each pixel to its nearest centroid
8. **Extend map**: `extend_palette_color_map` pads the index map to cover full block dimensions

#### Palette Cache

`svt_get_palette_cache_y`: Merges sorted palette color arrays from above and left neighbors into a combined sorted cache (max `2 * PALETTE_MAX_SIZE` entries). Used to reduce signaling cost for palette colors.

#### Palette Color Cost

`svt_av1_palette_color_cost_y`: Estimates bits needed to signal palette colors using delta encoding with the color cache.

Delta encoding:
- First color: `bit_depth` bits
- Subsequent colors: variable bits based on max delta, minimum `bit_depth - 3` bits per delta

### 10. Filter-Intra Mode

Five filter-intra modes (FILTER_DC_PRED, FILTER_V_PRED, FILTER_H_PRED, FILTER_D157_PRED, FILTER_PAETH_PRED) use a recursive 4x2 sub-block prediction with 7-tap filters.

#### Algorithm

The block is processed in 4x2 sub-blocks, scanning left-to-right, top-to-bottom:
```
Initialize buffer with above[-1..bw] and left[0..bh-1]

For r = 1, 3, 5, ... (step 2, row pairs):
  For c = 1, 5, 9, ... (step 4, column quads):
    p0 = buffer[r-1][c-1]  // above-left
    p1 = buffer[r-1][c]    // above[0]
    p2 = buffer[r-1][c+1]  // above[1]
    p3 = buffer[r-1][c+2]  // above[2]
    p4 = buffer[r-1][c+3]  // above[3]
    p5 = buffer[r][c-1]    // left[0]
    p6 = buffer[r+1][c-1]  // left[1]

    For k = 0..7:
      r_offset = k >> 2    // 0 for top row, 1 for bottom
      c_offset = k & 0x03  // 0-3 column within sub-block
      buffer[r+r_offset][c+c_offset] = clip(
        round(sum(taps[mode][k][j] * p_j for j in 0..6), FILTER_INTRA_SCALE_BITS))
```

`FILTER_INTRA_SCALE_BITS = 4`, so the sum of absolute tap weights should be approximately 16.

The tap coefficients are stored in `eb_av1_filter_intra_taps[FILTER_INTRA_MODES][8][8]` (the 8th tap element is padding; only 7 reference pixels are used).

Filter-intra is available only for blocks up to 32x32.

### 11. Reference Sample Assembly

`build_intra_predictors` / `build_intra_predictors_high` in `enc_intra_prediction.c`:

1. Determine which reference samples are needed based on mode and angle
2. Copy available samples from neighbor arrays into local `above_row` and `left_col` buffers
3. When reference samples are unavailable:
   - If opposite edge is available, extend with its corner sample
   - Otherwise, fill with `129` (left) or `127` (above) for LBD, or `base+1`/`base-1` for HBD where `base = 128 << (bd - 8)`
4. When partially available, extend last valid sample
5. Set `above_row[-1]` (above-left) from available corners
6. Apply edge filter and upsample if directional mode

### 12. Top-Right and Bottom-Left Availability

`svt_aom_intra_has_top_right` and `svt_aom_intra_has_bottom_left` determine whether reference pixels beyond the block boundary are available, based on:

- Block position within the superblock
- Block size and TX size
- Partition type
- Pre-computed lookup tables (`has_tr_*` / `has_bl_*`) encoding availability as bit flags

These tables encode the Z-order (raster scan within superblock) processing order to determine which blocks have been reconstructed.

## Key Functions

| Function | File | Purpose |
|----------|------|---------|
| `svt_av1_predict_intra_block` | enc_intra_prediction.c | Top-level entry: dispatches to build_intra_predictors, palette, or CfL |
| `build_intra_predictors` | enc_intra_prediction.c | LBD reference assembly, edge filter, mode dispatch |
| `build_intra_predictors_high` | enc_intra_prediction.c | HBD reference assembly, edge filter, mode dispatch |
| `svt_aom_dr_predictor` | intra_prediction.c | Directional prediction dispatch (z1/z2/z3) |
| `svt_av1_dr_prediction_z1_c` | intra_prediction.c | Zone 1 directional (0 < angle < 90) |
| `svt_av1_dr_prediction_z2_c` | intra_prediction.c | Zone 2 directional (90 < angle < 180) |
| `svt_av1_dr_prediction_z3_c` | intra_prediction.c | Zone 3 directional (180 < angle < 270) |
| `svt_av1_filter_intra_edge_c` | intra_prediction.c | 5-tap edge filter for reference samples |
| `svt_av1_filter_intra_predictor` | common_dsp_rtcd (via RTCD) | LBD filter-intra predictor |
| `svt_aom_highbd_filter_intra_predictor` | intra_prediction.c | HBD filter-intra predictor |
| `svt_cfl_luma_subsampling_420_lbd_c` | intra_prediction.c | CfL luma subsampling (LBD, 4:2:0) |
| `svt_cfl_luma_subsampling_420_hbd_c` | intra_prediction.c | CfL luma subsampling (HBD, 4:2:0) |
| `svt_subtract_average_c` | intra_prediction.c | CfL DC subtraction |
| `svt_cfl_predict_lbd_c` | (declared in header) | CfL chroma prediction (LBD) |
| `svt_cfl_predict_hbd_c` | (declared in header) | CfL chroma prediction (HBD) |
| `svt_aom_intra_edge_filter_strength` | intra_prediction.c | Determine edge filter strength |
| `svt_aom_use_intra_edge_upsample` | intra_prediction.c | Determine if edge upsample is needed |
| `svt_aom_intra_has_top_right` | intra_prediction.c | Top-right reference availability check |
| `svt_aom_intra_has_bottom_left` | intra_prediction.c | Bottom-left reference availability check |
| `filter_intra_edge_corner` | intra_prediction.c | Corner smoothing of above-left sample |
| `svt_aom_init_intra_predictors_internal` | intra_prediction.c | Initialize function pointer dispatch tables |
| `search_palette_luma` | palette.c | Palette mode search for luma |
| `palette_rd_y` | palette.c | Palette RD evaluation for luma |
| `av1_k_means` | palette.c | K-means clustering for palette derivation |
| `svt_get_palette_cache_y` | palette.c | Build palette color cache from neighbors |

## Dependencies

- **Block structures**: `BlockSize`, `TxSize`, `PredictionMode`, `FilterIntraMode` from `definitions.h`
- **Neighbor arrays**: `NeighborArrayUnit` for accessing reconstructed above/left samples
- **Mode decision**: Intra prediction is called from mode decision (`mode_decision.c`) and encoding loop
- **Transform sizes**: All TX_SIZES_ALL (19 sizes from 4x4 to 64x64 including rectangular)
- **Common utilities**: `ROUND_POWER_OF_TWO`, `clip_pixel`, `clip_pixel_highbd`
- **Palette mode** depends on: `k_means_template.h` (included via `#include` with dimension define)

## SIMD Functions

Most prediction kernels have SIMD-optimized versions dispatched via RTCD (Runtime CPU Dispatch):

| C Function | SIMD Variants | Notes |
|------------|---------------|-------|
| `svt_av1_dr_prediction_z1_c` | AVX2, SSE4.1, NEON | Zone 1 directional |
| `svt_av1_dr_prediction_z2_c` | AVX2, SSE4.1, NEON | Zone 2 directional |
| `svt_av1_dr_prediction_z3_c` | AVX2, SSE4.1, NEON | Zone 3 directional |
| `svt_av1_filter_intra_edge_c` | SSE4.1 | Edge filter |
| `svt_av1_upsample_intra_edge_c` | SSE4.1 | Edge upsample |
| `svt_av1_filter_intra_predictor_c` | SSE4.1 | Filter-intra |
| `svt_cfl_luma_subsampling_420_lbd_c` | AVX2 | CfL luma subsample |
| `svt_cfl_luma_subsampling_420_hbd_c` | AVX2 | CfL luma subsample HBD |
| `svt_subtract_average_*` | AVX2 | Per-size CfL subtract average |
| `svt_cfl_predict_lbd_c` | AVX2 | CfL predict LBD |
| `svt_cfl_predict_hbd_c` | AVX2 | CfL predict HBD |
| All sized predictors (`svt_aom_*_predictor_WxH`) | SSE2, SSSE3, AVX2, NEON | Per-size non-directional modes |
| `svt_aom_highbd_*_predictor_WxH` | SSE2, AVX2, NEON | Per-size HBD modes |
| `svt_av1_highbd_dr_prediction_z1_c` | AVX2, NEON | HBD zone 1 |
| `svt_av1_highbd_dr_prediction_z2_c` | AVX2, NEON | HBD zone 2 |
| `svt_av1_highbd_dr_prediction_z3_c` | AVX2, NEON | HBD zone 3 |
