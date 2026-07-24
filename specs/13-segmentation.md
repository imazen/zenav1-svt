# Segmentation

## Overview

The AV1 segmentation feature allows the encoder to partition a frame into up to 8 segments, each with independent coding parameters. In SVT-AV1, segmentation is used primarily for **variance-based adaptive quantization (AQ mode 1)** and **Region of Interest (ROI) maps**.

Each segment can override up to 8 coding features: quantizer, loop filter strength (4 directions), reference frame, skip mode, and global motion. SVT-AV1 currently uses only the quantizer (`SEG_LVL_ALT_Q`) feature for its variance-based AQ, and both quantizer and loop filter features for ROI maps.

The segment map assigns a segment ID to each coding block. During encoding, blocks in low-variance (flat) areas receive lower QP deltas (higher quality) while blocks in high-variance (textured) areas receive higher QP deltas, redistributing bits where they are perceptually most needed.

## Source Files

| File | Role | LOC (approx) |
|------|------|-------------|
| `Source/Lib/Codec/segmentation.c` | Segment map setup, QP binning, segment assignment | ~310 |
| `Source/Lib/Codec/segmentation.h` | Function declarations | ~30 |
| `Source/Lib/Codec/segmentation_params.c` | Feature metadata tables (bits, signs, max values) | ~20 |
| `Source/Lib/Codec/segmentation_params.h` | `SegmentationParams` struct, `SEG_LVL_FEATURES` enum, `SegmentationNeighborMap` | ~80 |

## Test Coverage

No dedicated unit tests exist for the segmentation module. Segmentation is exercised indirectly through encoder integration tests and through the bitstream conformance tests.

## Data Structures

### SegmentationParams (segmentation_params.h:35-75)

The per-frame segmentation configuration, stored in the frame header:

| Field | Type | Description |
|-------|------|-------------|
| `segmentation_enabled` | u8 | 1 if segmentation is active for this frame |
| `segmentation_update_map` | u8 | 1 if segment map is updated this frame |
| `segmentation_temporal_update` | u8 | 1 if map updates are coded relative to previous frame's map |
| `segmentation_update_data` | u8 | 1 if segment feature parameters are being updated |
| `feature_data[MAX_SEGMENTS][SEG_LVL_MAX]` | i16 | Per-segment feature values (e.g., QP delta for SEG_LVL_ALT_Q) |
| `feature_enabled[MAX_SEGMENTS][SEG_LVL_MAX]` | i16 | Per-segment feature enable flags |
| `seg_qm_level[MAX_SEGMENTS][SEG_LVL_MAX]` | i16 | Per-segment quantization matrix levels |
| `last_active_seg_id` | u8 | Highest segment ID with any enabled feature |
| `seg_id_pre_skip` | u8 | 1 if segment ID is read before skip syntax element |
| `variance_bin_edge[MAX_SEGMENTS]` | i16 | Variance thresholds for segment assignment (QP binning) |

### SEG_LVL_FEATURES Enum (segmentation_params.h:17-27)

| Value | Name | Meaning |
|-------|------|---------|
| 0 | `SEG_LVL_ALT_Q` | Alternate quantizer delta |
| 1 | `SEG_LVL_ALT_LF_Y_V` | Alternate loop filter, luma vertical |
| 2 | `SEG_LVL_ALT_LF_Y_H` | Alternate loop filter, luma horizontal |
| 3 | `SEG_LVL_ALT_LF_U` | Alternate loop filter, U plane |
| 4 | `SEG_LVL_ALT_LF_V` | Alternate loop filter, V plane |
| 5 | `SEG_LVL_REF_FRAME` | Constrained reference frame |
| 6 | `SEG_LVL_SKIP` | Force skip + zero motion |
| 7 | `SEG_LVL_GLOBALMV` | Force global motion |

### Feature Metadata Tables (segmentation_params.c)

| Table | Values | Description |
|-------|--------|-------------|
| `svt_aom_segmentation_feature_signed[8]` | `{1,1,1,1,1, 0,0,0}` | Whether feature values are signed |
| `svt_aom_segmentation_feature_bits[8]` | `{8,6,6,6,6, 3,0,0}` | Bits to code feature value |
| `svt_aom_segmentation_feature_max[8]` | `{MAXQ, MAX_LF, MAX_LF, MAX_LF, MAX_LF, 7, 0, 0}` | Maximum absolute feature value |

### SegmentationNeighborMap (segmentation_params.h:29-33)

| Field | Type | Description |
|-------|------|-------------|
| `dctor` | `EbDctor` | Destructor |
| `data` | u8* | Segment ID per 4x4 block |
| `map_size` | u32 | Total number of 4x4 blocks |

### Key Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `MAX_SEGMENTS` | 8 | Maximum number of segments in AV1 |
| `SEG_LVL_MAX` | 8 | Number of segment-level features |
| `MAXQ` | 255 | Maximum quantizer index |
| `MAX_LOOP_FILTER` | 63 | Maximum loop filter strength |

## Algorithms

### Algorithm 1: Variance-Based Segment QP Assignment (find_segment_qps)

**Purpose:** Divide blocks into segments based on their variance and assign QP offsets that give more bits to flat areas and fewer bits to busy areas.

**Input:** Per-SB 8x8 block variances (from motion estimation), base QP

**Steps:**

1. **Collect global variance statistics:**
   - Scan all 8x8 block variances across all 64x64 superblocks
   - Track `min_var`, `max_var`, and compute `avg_var` (average across all SBs)
   - `avg_var = log2(avg_var)` (log domain)

2. **Compute variance bin edges (in log domain):**
   ```
   min_var_log = log2(max(1, min_var))
   max_var_log = log2(max(1, max_var))
   step_size = (max_var_log - min_var_log) <= MAX_SEGMENTS ? 1 : round((max_var_log - min_var_log) / MAX_SEGMENTS)
   ```

3. **Assign QP offsets per segment (from highest segment down to 0):**
   ```
   bin_edge = min_var_log + step_size
   bin_center = bin_edge >> 1

   for i in (MAX_SEGMENTS-1) down to 0:
       variance_bin_edge[i] = 2^bin_edge
       feature_data[i][SEG_LVL_ALT_Q] = round(strength * (max(1, bin_center) - avg_var))
       bin_edge += step_size
       bin_center += step_size
   ```
   Where `strength = 2` (hardcoded tuning constant).

4. **Clamp segment 0:** If `feature_data[0][SEG_LVL_ALT_Q] < 0`, set it to 0 to avoid lossless blocks.

**Result:** Segments with low indices correspond to low-variance regions and get negative QP deltas (better quality). Segments with high indices correspond to high-variance regions and get positive QP deltas.

### Algorithm 2: Per-Block Segment ID Assignment (apply_segmentation_based_quantization)

**Purpose:** At block encoding time, look up the block's variance and assign the appropriate segment ID.

**Input:** Block position, block size, per-SB variance array, segment params

**Steps:**

1. **Get block variance** via `get_variance_for_cu`:
   - Map block size to the appropriate ME variance index tier:
     - 4x4/4x8/8x4/8x8: use `ME_TIER_ZERO_PU_8x8_*` indices
     - 16x16 and related: use `ME_TIER_ZERO_PU_16x16_*` indices
     - 32x32 and related: use `ME_TIER_ZERO_PU_32x32_*` indices
     - 64x64: use index 0
   - For non-square blocks that span two variance entries, average the two

2. **Find matching segment:** Scan from `MAX_SEGMENTS - 1` down to 0:
   ```
   if variance <= variance_bin_edge[i]:
       q_index = base_q_idx + feature_data[i][SEG_LVL_ALT_Q]
       if q_index > 0:   // avoid lossless
           segment_id = i
           break
   ```

3. The first segment whose variance threshold covers the block (scanning from highest to lowest) is assigned, provided it does not produce a lossless QP.

### Algorithm 3: ROI Map Segmentation (roi_map_setup_segmentation + roi_map_apply_segmentation_based_quantization)

**Purpose:** Allow external applications to define regions of interest with per-region quality control.

**Input:** `SvtAv1RoiMapEvt` containing a `b64_seg_map` (segment ID per 64x64 block) and `seg_qp` (QP delta per segment)

**Setup (roi_map_setup_segmentation):**

1. Enable segmentation with `update_map = true`, `update_data = true`, `temporal_update = false`.

2. For each segment `0..max_seg_id`:
   - Set `feature_data[i][SEG_LVL_ALT_Q] = roi_map->seg_qp[i]`
   - Enable loop filter features for all 4 directions

3. Compute loop filter deltas:
   - Get base filter levels from base QP via `svt_av1_pick_filter_level_by_q`
   - For each segment, get filter levels at `base_qp + seg_qp[i]`
   - Store the delta: `feature_data[i][SEG_LVL_ALT_LF_*] = segment_level - base_level`

**Assignment (roi_map_apply_segmentation_based_quantization):**

- For 64x64 SB: directly look up `b64_seg_map[row][col]`
- For 128x128 SB: check which of 4 constituent 64x64 blocks the current coding block overlaps, take the minimum segment ID across all overlapping 64x64 blocks
- Clamp to avoid lossless: scan from assigned segment down to 0 until `base_q + delta > 0`

### Algorithm 4: Segmentation Metadata Finalization (calculate_segmentation_data)

**Purpose:** Compute derived fields required for bitstream coding.

**Steps:**

1. Scan all segments and features to find:
   - `last_active_seg_id`: highest segment index with any enabled feature
   - `seg_id_pre_skip`: set to 1 if any segment has `SEG_LVL_REF_FRAME` or higher features enabled (these features affect parsing order)

## Key Functions

| Function | File | Description |
|----------|------|-------------|
| `svt_aom_setup_segmentation` | segmentation.c | Top-level: configure segmentation for a frame. Dispatches to ROI or variance-based path |
| `find_segment_qps` | segmentation.c | Compute variance bin edges and QP offsets for all 8 segments |
| `svt_aom_apply_segmentation_based_quantization` | segmentation.c | Assign segment ID to a coding block based on its variance or ROI map |
| `get_variance_for_cu` | segmentation.c | Map block position/size to ME variance tier and return variance |
| `calculate_segmentation_data` | segmentation.c | Compute `last_active_seg_id` and `seg_id_pre_skip` |
| `roi_map_setup_segmentation` | segmentation.c | Configure segmentation from external ROI map |
| `roi_map_apply_segmentation_based_quantization` | segmentation.c | Assign segment from ROI b64_seg_map |
| `temporally_update_qps` | segmentation.h (declared) | Temporal smoothing of segment QPs (declared but not in segmentation.c) |

## Dependencies

### Internal Dependencies

| Dependency | Used For |
|------------|----------|
| `pcs.h` / `PictureControlSet` | Per-picture state: variance arrays, frame header, b64 count |
| `sequence_control_set.h` | Encoder configuration: `aq_mode`, SB size, max input dimensions |
| `me_context.h` | `ME_TIER_ZERO_PU_*` constants for variance index mapping |
| `rc_process.h` | Rate control integration |
| `deblocking_filter.h` | `svt_av1_pick_filter_level_by_q` for ROI loop filter deltas |
| `utility.h` | `svt_log2f`, `ROUND`, `POW2`, `MIN`, `MAX`, `CLIP3` macros |
| `definitions.h` | `block_size_wide[]`, `block_size_high[]`, `BlockSize` enum |

### External Dependencies

None beyond standard C headers (indirectly via `<inttypes.h>` for debug logging).

## SIMD Functions

The segmentation module has no SIMD-optimized functions. All operations are scalar:
- Variance lookup is a simple array index
- QP computation uses integer arithmetic
- Segment assignment is a linear scan of 8 segments

The variance values consumed by segmentation are produced by the motion estimation module, which does have SIMD implementations, but that is outside the scope of this component.
