# Temporal Filtering

## Overview

Temporal filtering (TF) in SVT-AV1 produces denoised alt-ref frames by averaging
multiple source frames weighted by their similarity to a central frame. The filtered
result replaces the original source for encoding, reducing noise while preserving
detail. This is a pre-encoding operation that improves coding efficiency at the cost
of additional look-ahead processing.

The algorithm operates in 64x64 blocks with a hierarchical motion-compensated
structure. For each block:

1. Estimate noise level of the central frame (Sobel gradient + Laplacian).
2. Compute decay factors from noise, quantizer, and frame type.
3. For each reference frame in the temporal window:
   a. Run hierarchical motion estimation (HME + full-pel + sub-pel at 64x64, 32x32, 16x16, and optionally 8x8).
   b. Decide block partition (64x64 vs 4x 32x32 vs 16x 16x16 vs 64x 8x8) based on distortion comparison.
   c. Generate motion-compensated prediction using the chosen partition and MVs.
   d. Compute per-pixel filtering weights from block error, window error, motion distance, and decay factor.
   e. Accumulate weighted pixel values and weight counts.
4. Normalize accumulated values to produce the filtered output.

Two filtering paths exist:
- **Standard planewise** (`apply_temporal_filter_planewise_medium`): Uses both source and prediction to compute per-quadrant SSE (window error), combines with block-level ME error and motion distance for weight calculation.
- **Zero-motion planewise** (`apply_zz_based_temporal_filter_planewise_medium`): Simplified path for `use_zz_based_filter` mode that skips motion estimation and uses only block-level error.

A separate low-delay (LD) path (`produce_temporally_filtered_pic_ld`) uses zero-motion vectors without ME search.

All arithmetic uses fixed-point representations (FP8, FP10, FP16) to avoid floating-point operations in the hot path.

## Source Files

| File | Description |
|---|---|
| `Source/Lib/Codec/temporal_filtering.h` | Header: constants, TF_SUBPEL_SEARCH_PARAMS struct, public function declarations |
| `Source/Lib/Codec/temporal_filtering.c` | Main implementation (~3980 lines): noise estimation, ME setup, sub-pel search, filtering, normalization, pipeline integration |
| `Source/Lib/Codec/definitions.h` | `TfControls` struct definition (lines 159-234) |
| `Source/Lib/Codec/me_context.h` | `MeContext` TF fields: block errors, MVs, split flags, decay factors (lines 444-478) |
| `Source/Lib/Codec/pcs.h` | `PictureParentControlSet` TF fields: `tf_ctrls`, `past_altref_nframes`, `future_altref_nframes`, `temp_filt_*` |
| `Source/Lib/Codec/aom_dsp_rtcd.c` | Function pointer dispatch tables for all TF SIMD variants |
| `Source/Lib/Codec/aom_dsp_rtcd.h` | Declarations for dispatched function pointers |
| `Source/Lib/Codec/me_process.c` | Pipeline entry: calls `svt_av1_init_temporal_filtering` (line 306) |
| `Source/Lib/Codec/enc_mode_config.c` | `TfControls` configuration per encoder preset |
| `Source/Lib/ASM_AVX2/temporal_filtering_avx2.c` | AVX2 SIMD implementations (1086 lines) |
| `Source/Lib/ASM_SSE4_1/temporal_filtering_sse4_1.c` | SSE4.1 SIMD implementations (940 lines) |
| `Source/Lib/ASM_NEON/temporal_filtering_neon.c` | NEON SIMD implementations (1355 lines) |
| `Source/Lib/ASM_SVE/temporal_filtering_sve.c` | SVE implementation for `get_final_filtered_pixels` only |

## Test Coverage

| Test Class | What It Tests | Architectures |
|---|---|---|
| `TemporalFilterTestPlanewiseMedium` | `svt_av1_apply_temporal_filter_planewise_medium` (8-bit, with source) | SSE4.1, AVX2, NEON |
| `TemporalFilterZZTestPlanewiseMedium` | `svt_av1_apply_zz_based_temporal_filter_planewise_medium` (8-bit, zero-motion) | SSE4.1, AVX2, NEON |
| `TemporalFilterTestPlanewiseMediumHbd` | `svt_av1_apply_temporal_filter_planewise_medium_hbd` (10-bit, with source) | SSE4.1, AVX2, NEON |
| `TemporalFilterZZTestPlanewiseMediumHbd` | `svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd` (10-bit, zero-motion) | SSE4.1, AVX2, NEON |
| `TemporalFilterTestGetFinalFilteredPixels` | `get_final_filtered_pixels` (normalization) | SSE4.1, AVX2, NEON, SVE |
| `TemporalFilterTestApplyFilteringCentralLbd` | `apply_filtering_central` (8-bit central frame) | SSE4.1, AVX2, NEON |
| `TemporalFilterTestApplyFilteringCentralHbd` | `apply_filtering_central_highbd` (10-bit central frame) | SSE4.1, AVX2, NEON |
| `EstimateNoiseTestFP` | `svt_estimate_noise_fp16` (8-bit noise estimation) | AVX2, NEON |
| `EstimateNoiseTestFPHbd` | `svt_estimate_noise_highbd_fp16` (10-bit noise estimation) | AVX2, NEON |

All SIMD tests compare output bit-exactly against the C reference. Each test also has a `DISABLED_Speed` variant for benchmarking.

## Data Structures

### TfControls (definitions.h:159-234)

Controls all temporal filtering behavior. Set per encoder preset in `enc_mode_config.c` and copied to `MeContext` at filtering time.

```
TfControls {
    // Filtering set
    uint8_t  enabled;                // 0: OFF, 1: ON
    uint8_t  chroma_lvl;             // 0: luma only, 1: all planes, 2: conditional on noise
    uint8_t  use_zz_based_filter;    // 0: use ME, 1: use (0,0) MVs

    // Reference frame count
    uint8_t  num_past_pics;          // default past frames
    uint8_t  num_future_pics;        // default future frames
    uint8_t  modulate_pics;          // adjust count based on noise/distortion
    uint8_t  use_intra_for_noise_est;// reuse keyframe noise level
    uint8_t  max_num_past_pics;      // upper bound after adjustment
    uint8_t  max_num_future_pics;    // upper bound after adjustment

    // Motion search controls
    uint8_t  hme_me_level;           // ME accuracy level
    uint8_t  half_pel_mode;          // 0: OFF, 1: full 8-pos, 2/3: axis-only
    uint8_t  quarter_pel_mode;       // 0: OFF, 1: full 8-pos, 2/3: axis-only
    uint8_t  eight_pel_mode;         // 0: OFF, 1: full 8-pos
    uint8_t  use_8bit_subpel;        // do 10-bit subpel search in 8-bit
    uint8_t  avoid_2d_qpel;          // skip diagonal quarter-pel positions
    uint8_t  use_2tap;               // bilinear instead of 8-tap for subpel
    uint8_t  sub_sampling_shift;     // row subsampling for distortion calc
    uint64_t pred_error_32x32_th;    // skip 16x16 subpel if 32x32 error below this
    bool     enable_8x8_pred;        // enable 8x8 block partition

    // Early exit / skip controls
    uint32_t me_exit_th;             // skip full-pel if HME dist below this
    uint8_t  use_pred_64x64_only_th; // use only 64x64 prediction threshold
    uint8_t  subpel_early_exit_th;   // exit subpel if dist/variance is low
    uint8_t  ref_frame_factor;       // frame skip factor (1=all, 2=every other, etc.)
    uint8_t  qp_opt;                 // QP-based parameter tuning
}
```

### TF_SUBPEL_SEARCH_PARAMS (temporal_filtering.h:102-119)

Parameters for one sub-pel search position check:

```
TF_SUBPEL_SEARCH_PARAMS {
    uint8_t      subpel_pel_mode;    // search mode (matches half/quarter/eight_pel_mode)
    signed short xd, yd;             // delta from base MV (in 1/8 pel units)
    signed short mv_x, mv_y;         // base MV (in 1/8 pel units)
    uint32_t     interp_filters;     // interpolation filter pair
    uint16_t     pu_origin_x, pu_origin_y;     // absolute block origin
    uint16_t     local_origin_x, local_origin_y; // origin within the 64x64 SB
    uint32_t     bsize;              // block size (8, 16, 32, or 64)
    uint8_t      is_highbd;          // high bit depth flag
    uint8_t      encoder_bit_depth;  // 8 or 10
    uint8_t      subsampling_shift;  // row subsampling for distortion
    uint32_t     idx_x, idx_y;       // block index within the 64x64 grid
}
```

### MeContext TF Fields (me_context.h:444-478)

Per-block filtering state carried through the ME/TF pipeline:

```
// Per-plane decay factors (computed from noise, QP, frame type)
uint32_t tf_decay_factor_fp16[MAX_PLANES];  // FP16 fixed-point

// 64x64 level
int16_t  tf_64x64_mv_x, tf_64x64_mv_y;    // best MV in 1/8 pel
uint64_t tf_64x64_block_error;              // distortion

// 32x32 level (4 blocks per 64x64)
int16_t  tf_32x32_mv_x[4], tf_32x32_mv_y[4];
uint64_t tf_32x32_block_error[4];
int      tf_32x32_block_split_flag[4];      // 1 = split to 16x16

// 16x16 level (16 blocks per 64x64)
int16_t  tf_16x16_mv_x[16], tf_16x16_mv_y[16];
uint64_t tf_16x16_block_error[16];
int      tf_16x16_block_split_flag[4][4];   // [32x32_idx][16x16_idx] -> split to 8x8

// 8x8 level (64 blocks per 64x64)
int16_t  tf_8x8_mv_x[64], tf_8x8_mv_y[64];
uint64_t tf_8x8_block_error[64];

// Current processing position
int      tf_block_row, tf_block_col;        // which 32x32 quadrant
uint32_t idx_32x32;                         // flat index
uint16_t tf_mv_dist_th;                     // distance threshold for weight adjustment
uint8_t  tf_chroma;                         // filter chroma planes
```

### Accumulator/Counter Buffers

Per-block working buffers, stack-allocated and 16-byte aligned:

```
DECLARE_ALIGNED(16, uint32_t, accumulator[BLK_PELS * MAX_PLANES]);  // 4096 * 3
DECLARE_ALIGNED(16, uint16_t, counter[BLK_PELS * MAX_PLANES]);      // 4096 * 3
```

`BLK_PELS` = 4096 (64x64), `MAX_PLANES` = 3 (Y, U, V).

For each pixel position, `accum[plane][k]` holds the weighted sum of pixel values, and `count[plane][k]` holds the sum of weights. The final filtered pixel = `accum[k] / count[k]`.

### Block Index Mapping Tables

Static tables map between block hierarchy levels:

- `subblock_xy_16x16[16][2]`: (row, col) coordinates for each 16x16 block within a 64x64
- `subblock_xy_8x8[64][2]`: (row, col) coordinates for each 8x8 block within a 64x64
- `idx_32x32_to_idx_16x16[4][4]`: maps 32x32 block + sub-index to 16x16 flat index
- `idx_32x32_to_idx_8x8[4][4][4]`: maps 32x32 + 16x16 + 8x8 sub-indices to 8x8 flat index

## Algorithms

### 1. Noise Estimation

**Function**: `svt_estimate_noise_fp16_c` / `svt_estimate_noise_highbd_fp16_c`

For each pixel (excluding 1-pixel border):

1. Compute Sobel gradient magnitude: `ga = |g_x| + |g_y|`
   - `g_x = (src[k-stride-1] - src[k-stride+1]) + (src[k+stride-1] - src[k+stride+1]) + 2*(src[k-1] - src[k+1])`
   - `g_y = (src[k-stride-1] - src[k+stride-1]) + (src[k-stride+1] - src[k+stride+1]) + 2*(src[k-stride] - src[k+stride])`
2. If `ga < EDGE_THRESHOLD` (50), compute Laplacian:
   - `v = 4*src[k] - 2*(src[k-1] + src[k+1] + src[k-stride] + src[k+stride]) + (src[k-stride-1] + src[k-stride+1] + src[k+stride-1] + src[k+stride+1])`
   - Accumulate `sum += abs(v)`, `num++`
3. If `num < SMOOTH_THRESHOLD` (16), return -1 (unreliable).
4. Return `(sum * SQRT_PI_BY_2_FP16) / (6 * num)` in FP16.

For HBD, gradients and Laplacian values are scaled down by `2^(bd-8)` using `ROUND_POWER_OF_TWO`.

**Log transform**: `svt_aom_noise_log1p_fp16` converts noise level to log1p using a 224-entry lookup table (`log1p_tab_fp16`) with linear interpolation, covering range [0, 7) in steps of 1/32. Values >= 7 use a linear approximation.

### 2. Decay Factor Computation

**Function**: `svt_av1_calculate_decay_factor`

The decay factor controls overall filter strength per plane. Computed once per 64x64 block.

```
decay_control = {3, 6, 6} for {Y, U, V}  (or {1,1,1} for VQ/sharp mode)

// Noise-based decay
n_decay_fp10 = decay_control[plane] * (0.7 + log1p(noise_level)) / 64

// QP-based decay
if q >= TF_QINDEX_CUTOFF (128):
    q_decay_fp8 = q^2 / 32
else:
    q_decay_fp8 = max(q * 4, 1)

// Combined
tf_decay_factor_fp16 = n_decay^2 * q_decay >> shift_factor
```

The `shift_factor` (10-14) is derived from the 64x64 block error or manually set via `--tf-strength`:
- `block_error < 200`:  shift = 14 (weakest filtering)
- `block_error < 2000`: shift = 13
- otherwise:            shift = 12

For keyframes, shift is incremented by 1. When shift reaches 14, TF is disabled (decay = 0).

### 3. Motion Estimation and Sub-Pel Refinement

For each reference frame (not the center frame), per 64x64 block:

**a. Integer-pel ME** (`svt_aom_motion_estimation_b64`):
- Uses the standard HME pipeline: hierarchical search at 1/16, 1/4, and full resolution.
- Produces best MVs at 64x64, 32x32, 16x16, and 8x8 granularities.

**b. Sub-pel refinement** (per block size, largest first):
- `tf_64x64_sub_pel_search`: Refines the 64x64 MV.
- `tf_32x32_sub_pel_search`: Refines each 32x32 MV (4 per 64x64).
- `tf_16x16_sub_pel_search`: Refines each 16x16 MV (conditional on 32x32 error > `pred_error_32x32_th`).
- `tf_8x8_sub_pel_search`: Refines each 8x8 MV (conditional on `enable_8x8_pred`).

**Sub-pel search** (`tf_subpel_search`): Three-stage diamond search:
1. **Half-pel** (step=4 in 1/8 units): 8 positions around best MV (or 4 axis-only positions if `half_pel_mode >= 2`).
2. **Quarter-pel** (step=2): 8 positions around refined MV.
3. **Eighth-pel** (step=1): 8 positions around refined MV.

Each position is evaluated by `svt_check_position`:
1. Skip diagonal positions if `subpel_pel_mode >= 2`.
2. Early exit if best distortion is already 0.
3. Early exit if per-pixel distortion < `subpel_early_exit_th`.
4. Generate luma prediction via `svt_aom_simple_luma_unipred`.
5. Compute variance-based distortion using `svt_aom_mefn_ptr` function pointers.
6. Update best MV if distortion improves.

**c. 64x64 vs 32x32 decision**:
- If `use_pred_64x64_only_th` is set and deviation between 64x64 SAD and sum of 32x32 SADs is small, use 64x64 only.
- Otherwise, if `64x64_error * 14 < sum_32x32_error * 16` and `64x64_error < 2^18`, use 64x64 prediction.
- Otherwise, proceed to 32x32 / 16x16 / 8x8 partitioning.

### 4. Block Partition Decision

**Function**: `derive_tf_32x32_block_split_flag`

For each 32x32 block, decides whether to use a single 32x32 MV or split to four 16x16 MVs (and optionally further to 8x8):

**32x32 split decision**:
```
if block_error_32x32 * 14 < sum_16x16_errors * 16:
    no split (use 32x32 MV)
else:
    split (use 16x16 MVs)
```

**16x16 split decision** (when `enable_8x8_pred` is true):
```
error_8x8 = sum of 4 child 8x8 errors
if error_16x16 * 8 < error_8x8 * 16:
    no split
else:
    split (use 8x8 MVs)
    update 16x16 error = error_8x8
```

The threshold ratios (14/16 for 32x32, 8/16 for 16x16) encode a bias toward larger blocks unless the smaller blocks provide meaningfully better alignment.

### 5. Motion-Compensated Prediction

**Functions**: `tf_64x64_inter_prediction`, `tf_32x32_inter_prediction`

After partition decision, generates the actual motion-compensated prediction:

- For unsplit 32x32: single call to `svt_aom_inter_prediction` with BLOCK_32X32.
- For split 32x32 with unsplit 16x16: calls `svt_aom_inter_prediction` with BLOCK_16X16 for each sub-block.
- For split 16x16: calls `svt_aom_inter_prediction` with BLOCK_8X8 for each sub-block.

Uses `MULTITAP_SHARP` interpolation filter for the final prediction (vs `EIGHTTAP_REGULAR` or `BILINEAR` during sub-pel search).

### 6. Weight Computation (Standard Planewise)

**Function**: `svt_av1_apply_temporal_filter_planewise_medium_partial_c`

For each 32x32 quadrant (processed as four subblocks of the current block):

**a. Window error** (per-quadrant SSE between source and prediction):
```
window_error_quad_fp8[q] = SSE(src_quad, pred_quad) * 256 / (width/2 * height/2)
```
For chroma, the window error blends chroma and luma: `chroma_err = (5 * chroma_err + luma_err) / 6`.

**b. Motion distance factor**:
```
distance = sqrt(mv_x^2 + mv_y^2)
distance_threshold = max(frame_height * 0.1, 1)
d_factor = max(distance / distance_threshold, 1)
```

**c. Combined error**:
```
combined_error = (window_error * TF_WINDOW_BLOCK_BALANCE_WEIGHT + block_error)
               / (TF_WINDOW_BLOCK_BALANCE_WEIGHT + 1)
// TF_WINDOW_BLOCK_BALANCE_WEIGHT = 5, so window error is 5x more important
```

**d. Scaled difference and weight**:
```
scaled_diff = min(combined_error * d_factor / tf_decay_factor, 7)
adjusted_weight = exp(-scaled_diff) * TF_WEIGHT_SCALE
```

The exponential is computed via a 128-entry lookup table (`expf_tab_fp16`) with FP16 precision, indexed by `scaled_diff * 16`.

**e. Accumulation**:
```
for each pixel (i, j) in subblock:
    count[k] += adjusted_weight
    accum[k] += adjusted_weight * prediction_pixel[k]
```

### 7. Weight Computation (Zero-Motion Planewise)

**Function**: `svt_av1_apply_zz_based_temporal_filter_planewise_medium_partial_c`

Simplified version when motion estimation is skipped (`use_zz_based_filter = 1`):
- No window error computation (no source comparison).
- No distance factor (motion is zero).
- Uses only block-level error from the partition decision.

```
avg_err_fp10 = block_error_fp8 << 2
scaled_diff16 = min(avg_err / max(tf_decay_factor >> 10, 1), 7*16)
adjusted_weight = expf_tab_fp16[scaled_diff16] * TF_WEIGHT_SCALE >> 17
```

### 8. Central Frame Contribution

**Function**: `svt_aom_apply_filtering_central_c` / `svt_aom_apply_filtering_central_highbd_c`

The central frame (the one being filtered) always receives weight `TF_PLANEWISE_FILTER_WEIGHT_SCALE` (1000):

```
for each pixel:
    accum[k] = 1000 * src_pixel[k]
    count[k] = 1000
```

This is applied first, before any reference frame contributions are accumulated on top.

### 9. Normalization (Final Filtered Pixels)

**Function**: `svt_aom_get_final_filtered_pixels_c`

After all reference frames have been accumulated:

```
filtered_pixel[k] = OD_DIVU(accum[k] + count[k]/2, count[k])
```

`OD_DIVU` is a fast integer division with rounding. The `count[k]/2` term provides rounding to nearest.

For 8-bit: result is cast to `uint8_t` and written to the source buffer.
For HBD: result is cast to `uint16_t` and written to the altref HBD buffer, later unpacked to the split 8+2 format.

### 10. Reference Frame Selection and Outlier Rejection

Before processing each reference frame:

**AHD error check**: Skip if the frame's absolute histogram difference (AHD) to the central frame is both high in absolute terms (> `aligned_width * aligned_height`) and significantly above the average AHD error by more than a percentage threshold (20% for I-slices, 40% for others).

**Brightness change check**: Count regions where average intensity differs by > 2 and average luma differs from central. If >= 14/16 of all regions show brightness change, skip the frame.

**Frame skip factor**: `ref_frame_factor` allows using every Nth reference frame (1 = all, 2 = every other, etc.).

## Key Functions

### Entry Point

| Function | Description |
|---|---|
| `svt_av1_init_temporal_filtering` | Main entry point. Called from `me_process.c`. Sets up TF controls, pads reference frames, packs HBD buffers, saves original source, dispatches to `produce_temporally_filtered_pic` or `produce_temporally_filtered_pic_ld`, post-processes (unpack HBD, pad, decimate). |

### Core Pipeline

| Function | Description |
|---|---|
| `produce_temporally_filtered_pic` | Random-access mode. Full ME + hierarchical sub-pel + block partition + filtering for each 64x64 block. |
| `produce_temporally_filtered_pic_ld` | Low-delay mode. Zero-motion only, no ME search. |
| `create_me_context_and_picture_control` | Sets up ME context for one reference frame: loads padded/quarter/sixteenth buffers, sets search method. |

### Motion Search

| Function | Description |
|---|---|
| `tf_64x64_sub_pel_search` | Sub-pel refinement for 64x64 block. |
| `tf_32x32_sub_pel_search` | Sub-pel refinement for one 32x32 block. |
| `tf_16x16_sub_pel_search` | Sub-pel refinement for four 16x16 blocks within a 32x32. |
| `tf_8x8_sub_pel_search` | Sub-pel refinement for sixteen 8x8 blocks within a 32x32. |
| `tf_subpel_search` | Three-stage (half/quarter/eighth) sub-pel search loop. |
| `svt_check_position` | Evaluate one sub-pel position: generate prediction, compute distortion, update best. |
| `tf_use_64x64_pred` | Check if 64x64-only prediction is sufficient (compares 64x64 SAD vs sum of 32x32 SADs). |

### Partition Decision

| Function | Description |
|---|---|
| `derive_tf_32x32_block_split_flag` | Decides 32x32 -> 16x16 and 16x16 -> 8x8 splits based on error ratios. |
| `convert_64x64_info_to_32x32_info` | Propagates 64x64 MV and error info down to 32x32 level when 64x64-only mode is used. |

### Inter Prediction

| Function | Description |
|---|---|
| `tf_64x64_inter_prediction` | Generates MC prediction for entire 64x64 block using final refined MV. |
| `tf_32x32_inter_prediction` | Generates MC prediction respecting the block partition (32x32 / 16x16 / 8x8). |

### Filtering (Dispatched via RTCD)

| Function | Description |
|---|---|
| `svt_av1_apply_temporal_filter_planewise_medium` | Standard planewise filter: computes window SSE, combines with block error and distance factor, accumulates weighted pixels. 8-bit. |
| `svt_av1_apply_temporal_filter_planewise_medium_hbd` | Same as above for 10-bit. |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium` | Zero-motion planewise filter: block error only, no window SSE or distance factor. 8-bit. |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd` | Same as above for 10-bit. |
| `apply_filtering_central` | Central frame contribution (weight = 1000). 8-bit. |
| `apply_filtering_central_highbd` | Same for 10-bit. |
| `get_final_filtered_pixels` | Normalization: divides accumulated values by counts. |
| `apply_filtering_block_plane_wise` | Dispatcher: selects standard vs ZZ-based, 8-bit vs HBD, sets up buffer offsets. |

### Noise Estimation (Dispatched via RTCD)

| Function | Description |
|---|---|
| `svt_estimate_noise_fp16` | Sobel + Laplacian noise estimation. 8-bit. Returns FP16. |
| `svt_estimate_noise_highbd_fp16` | Same for 10-bit. |
| `svt_aom_noise_log1p_fp16` | Log1p of noise level using lookup table. |

### Post-Processing

| Function | Description |
|---|---|
| `pad_and_decimate_filtered_pic` | Pads filtered picture borders, generates 1/4 and 1/16 downsampled versions. |
| `save_src_pic_buffers` | Saves original unfiltered source (all planes) for PSNR/SSIM computation. |
| `save_y_src_pic_buffers` | Saves original unfiltered source (luma only) for I-slice distortion measurement. |
| `filt_unfilt_dist` | Computes per-SB average distortion between filtered and unfiltered pictures. |
| `set_hme_search_params_mctf` | Configures HME search area for MCTF (level 0: default, level 1: 2x-4x larger). |

### Helper Functions

| Function | Description |
|---|---|
| `calculate_squared_errors_sum` | Computes SSE between two 8-bit blocks. |
| `calculate_squared_errors_sum_highbd` | Computes SSE between two 16-bit blocks, with bit-depth shift. |
| `svt_av1_calculate_decay_factor` | Computes per-plane decay factors from noise, QP, and shift factor. |
| `calculate_tf_shift_factor` | Maps 64x64 block error to shift factor (12, 13, or 14). |
| `sqrt_fast` | Fast integer square root using a 16-entry FP16 lookup table with linear interpolation. Max error 10%. |

## Dependencies

| Dependency | Usage |
|---|---|
| `motion_estimation.h` / `svt_aom_motion_estimation_b64` | Integer-pel motion search |
| `enc_inter_prediction.h` / `svt_aom_inter_prediction` | Final MC prediction generation |
| `av1me.h` / `svt_aom_simple_luma_unipred` | Luma-only prediction for sub-pel search |
| `compute_sad.h` | SAD computation |
| `pic_operators.h` | Picture buffer operations (copy, pad) |
| `pic_analysis_process.h` | Downsampling of filtered picture |
| `lambda_rate_tables.h` | `quantizer_to_qindex`, QP conversion |
| `ac_bias.h` | `svt_av1_convert_qindex_to_q_fp8`, `svt_av1_compute_qdelta_fp` |
| `pack_unpack_c.h` | HBD pack/unpack between 8+2 split and 16-bit formats |

## SIMD Functions

### x86: SSE4.1 (temporal_filtering_sse4_1.c, 940 lines)

| Function | Description |
|---|---|
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_sse4_1` | ZZ-based filter, 8-bit |
| `svt_av1_apply_temporal_filter_planewise_medium_sse4_1` | Standard planewise filter, 8-bit |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_sse4_1` | ZZ-based filter, 10-bit |
| `svt_av1_apply_temporal_filter_planewise_medium_hbd_sse4_1` | Standard planewise filter, 10-bit |
| `svt_aom_get_final_filtered_pixels_sse4_1` | Normalization |
| `svt_aom_apply_filtering_central_sse4_1` | Central frame, 8-bit |
| `svt_aom_apply_filtering_central_highbd_sse4_1` | Central frame, 10-bit |

### x86: AVX2 (temporal_filtering_avx2.c, 1086 lines)

| Function | Description |
|---|---|
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_avx2` | ZZ-based filter, 8-bit |
| `svt_av1_apply_temporal_filter_planewise_medium_avx2` | Standard planewise filter, 8-bit |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_avx2` | ZZ-based filter, 10-bit |
| `svt_av1_apply_temporal_filter_planewise_medium_hbd_avx2` | Standard planewise filter, 10-bit |
| `svt_aom_get_final_filtered_pixels_avx2` | Normalization |
| `svt_aom_apply_filtering_central_avx2` | Central frame, 8-bit |
| `svt_aom_apply_filtering_central_highbd_avx2` | Central frame, 10-bit |
| `svt_estimate_noise_fp16_avx2` | Noise estimation, 8-bit |
| `svt_estimate_noise_highbd_fp16_avx2` | Noise estimation, 10-bit |

AVX2 helper: `calculate_squared_errors_sum_no_div_avx2` processes 16 pixels per iteration using `_mm256_cvtepu8_epi16`, `_mm256_sub_epi16`, `_mm256_madd_epi16`. Also has a dual-8xH variant for computing two 8-wide SSE values simultaneously.

### ARM: NEON (temporal_filtering_neon.c, 1355 lines)

| Function | Description |
|---|---|
| `svt_av1_apply_temporal_filter_planewise_medium_neon` | Standard planewise filter, 8-bit |
| `svt_av1_apply_temporal_filter_planewise_medium_hbd_neon` | Standard planewise filter, 10-bit |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_neon` | ZZ-based filter, 8-bit |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd_neon` | ZZ-based filter, 10-bit |
| `svt_aom_get_final_filtered_pixels_neon` | Normalization |
| `svt_aom_apply_filtering_central_neon` | Central frame, 8-bit |
| `svt_aom_apply_filtering_central_highbd_neon` | Central frame, 10-bit |
| `svt_estimate_noise_fp16_neon` | Noise estimation, 8-bit |
| `svt_estimate_noise_highbd_fp16_neon` | Noise estimation, 10-bit |

### ARM: SVE (temporal_filtering_sve.c)

| Function | Description |
|---|---|
| `svt_aom_get_final_filtered_pixels_sve` | Normalization using SVE vector length agnostic instructions |

The SVE variant only covers `get_final_filtered_pixels`; all other TF functions use the NEON paths on SVE-capable hardware.

### RTCD Dispatch Summary

All dispatched via `aom_dsp_rtcd.c` using `SET_SSE41_AVX2` / `SET_NEON` / `SET_NEON_SVE` macros:

| Logical Function | C | SSE4.1 | AVX2 | NEON | SVE |
|---|---|---|---|---|---|
| `svt_av1_apply_temporal_filter_planewise_medium` | Y | Y | Y | Y | - |
| `svt_av1_apply_temporal_filter_planewise_medium_hbd` | Y | Y | Y | Y | - |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium` | Y | Y | Y | Y | - |
| `svt_av1_apply_zz_based_temporal_filter_planewise_medium_hbd` | Y | Y | Y | Y | - |
| `get_final_filtered_pixels` | Y | Y | Y | Y | Y |
| `apply_filtering_central` | Y | Y | Y | Y | - |
| `apply_filtering_central_highbd` | Y | Y | Y | Y | - |
| `svt_estimate_noise_fp16` | Y | - | Y | Y | - |
| `svt_estimate_noise_highbd_fp16` | Y | - | Y | Y | - |

### Fixed-Point Lookup Tables

Both C and SIMD paths share these tables (duplicated in each SIMD file):

- `expf_tab_fp16[128]`: `exp(-x/16)` for x in [0..7] at step 1/16, in FP16. Used to convert scaled difference to filter weight.
- `sqrt_array_fp16[16]`: `sqrt(i) * 65536` for i in [0..15]. Used by `sqrt_fast` for distance computation.
- `log1p_tab_fp16[224]`: `log1p(x)` for x in [-1..6] at step 1/32, in FP16. Used for noise level log transform (C path only).
