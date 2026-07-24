# Picture Management & Pipeline

## Overview

SVT-AV1's picture management subsystem controls the lifecycle of every frame from input ingestion through encoding to output packetization. The system is organized around two central data structures -- `PictureParentControlSet` (PPCS) and `PictureControlSet` (PCS) -- that carry metadata and results through a multi-threaded pipeline of process kernels connected by FIFO queues.

The pipeline flow is:

```
Input Buffer
    |
    v
Resource Coordination  --> allocates PPCS, copies input, resets state
    |
    v
Picture Analysis       --> pads, downsamples, computes variance/histograms
    |
    v
Picture Decision       --> scene change detection, mini-GOP construction, reference assignment
    |
    v
Picture Manager        --> allocates child PCS + EncDecSet, waits for references, dispatches
    |
    v
[Mode Decision / Encode / Entropy Coding]
    |
    v
Packetization          --> assembles bitstream TUs, signals reference release, outputs packets
```

Each transition between stages uses a System Resource Manager (SRM) with producer/consumer FIFOs that enforce ordering and provide backpressure when pools are exhausted.

## Source Files

| File | Size | Description |
|------|------|-------------|
| `pcs.h` | 50 KB | Defines `PictureControlSet`, `PictureParentControlSet`, `EncDecSet`, `B64Geom`, and all per-picture control structures |
| `pcs.c` | 77 KB | Constructors/destructors for PCS, PPCS, EncDecSet; ME result allocation; neighbor array creation |
| `pic_buffer_desc.h` | 6 KB | `EbPictureBufferDesc` and `Yv12BufferConfig` structures; buffer init data |
| `pic_buffer_desc.c` | 16 KB | Buffer allocation (normal, split-mode 10-bit, recon); `svt_aom_realloc_frame_buffer`; EB-to-AOM linking |
| `pic_operators.h` | 4 KB | Picture operation function declarations (residual, distortion, packing, padding, copy) |
| `pic_operators.c` | 16 KB | Residual kernel, distortion computation, padding, pack/unpack 8/10/16-bit, YV12 copy |
| `pic_manager_process.h` | 1 KB | `PictureManagerContext` with min-heap for decode order tracking |
| `pic_manager_process.c` | 30+ KB | Picture Manager kernel: reference availability check, child PCS allocation, tile/segment init |
| `pic_analysis_process.h` | 1 KB | Declares `svt_aom_picture_analysis_kernel`, downsampling, padding functions |
| `pic_analysis_process.c` | 85 KB | Downsampling, variance/histogram computation, film grain, color counting, picture analysis kernel |
| `pic_analysis_results.h` | 0.5 KB | `PictureAnalysisResults` message struct |
| `pic_analysis_results.c` | 0.5 KB | Trivial constructor |
| `pic_demux_results.h` | 1 KB | `PictureDemuxResults` with `EbPicType` enum (INPUT, REFERENCE, FEEDBACK, SUPERRES_INPUT) |
| `pic_demux_results.c` | 0.5 KB | Trivial constructor |
| `pic_manager_queue.h` | 1.5 KB | `InputQueueEntry` and `ReferenceQueueEntry` for picture manager queues |
| `pic_manager_queue.c` | 0.5 KB | Trivial constructors |
| `pd_process.h` | 3 KB | `PictureDecisionContext` with mini-GOP state, DPB, scene change history |
| `pd_process.c` | 261 KB | Picture Decision kernel: scene change, mini-GOP, reference assignment, temporal filtering |
| `src_ops_process.h` | 1 KB | Source-based operations and TPL dispenser declarations |
| `src_ops_process.c` | 110 KB | TPL dispenser kernel, source-based operations kernel |
| `packetization_process.h` | 0.5 KB | Packetization kernel declaration |
| `packetization_process.c` | 25+ KB | Packetization kernel: reorder queue, TU assembly, EOS handling, reference release |
| `resource_coordination_process.h` | 0.5 KB | Resource coordination declaration |
| `resource_coordination_process.c` | 35+ KB | Resource coordination kernel: input copy, PPCS init, speed control, resolution change |
| `reference_object.h` | 3 KB | `EbReferenceObject`, `EbPaReferenceObject`, `EbTplReferenceObject` |
| `neighbor_arrays.h` | 3 KB | `NeighborArrayUnit` / `NeighborArrayUnit32` structures and access functions |
| `neighbor_arrays.c` | 6 KB | Neighbor array construction, reset, read/write operations |
| `resource_coordination_results.h` | 1 KB | `ResourceCoordinationResults` message struct (PPCS wrapper) and `InputCommand` struct passed from resource coordination to picture analysis |
| `resource_coordination_results.c` | 0.5 KB | Trivial constructor/creator for `ResourceCoordinationResults` |
| `initial_rc_reorder_queue.h` | 0.5 KB | Header for initial rate control reorder queue (includes `definitions.h`) |
| `initial_rc_reorder_queue.c` | 0.5 KB | Stub compilation unit for initial rate control reorder queue |
| `initial_rc_results.h` | 1 KB | `InitialRateControlResults` message struct (PPCS wrapper + superres recode flag) |
| `initial_rc_results.c` | 0.5 KB | Trivial constructor/creator for `InitialRateControlResults` |
| `pd_queue.h` | 1 KB | `PaReferenceEntry` for Picture Decision's pre-analysis reference DPB (picture number, decode order, validity flag) |
| `pd_queue.c` | 0.5 KB | Constructor for `PaReferenceEntry` |
| `pd_reorder_queue.h` | 1 KB | `PictureDecisionReorderEntry` for display-to-decode order reordering in Picture Decision |
| `pd_reorder_queue.c` | 0.5 KB | Constructor for `PictureDecisionReorderEntry` |
| `pd_results.h` | 1.5 KB | `PictureDecisionResults` message struct with ME/TF task type, reference lists, screen content flags, and downscaled reference pointers |
| `pd_results.c` | 0.5 KB | Trivial constructor/creator for `PictureDecisionResults` |
| `packetization_reorder_queue.h` | 2 KB | `PacketizationReorderEntry` for output reorder queue: bitstream, POC, frame type, reference signal, show-existing-frame metadata |
| `packetization_reorder_queue.c` | 0.5 KB | Constructor for `PacketizationReorderEntry` (allocates bitstream object) |
| `enc_dec_tasks.h` | 1 KB | `EncDecTasks` message struct with PCS wrapper, input type (MDC/EncDec/Continue/Superres), segment row, tile group index |
| `enc_dec_tasks.c` | 0.5 KB | Trivial constructor/creator for `EncDecTasks` |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| `test/PictureOperatorTest.cc` | `svt_aom_downsample_2d` (2x, 4x, 8x decimation against C reference), tested across multiple resolutions (1920x1080, 960x540, 176x144, 88x72) and with SIMD variants |

## Data Structures

### EbPictureBufferDesc

The fundamental image buffer. All picture data in the encoder flows through instances of this structure.

**Fields:**
- `y_buffer`, `u_buffer`, `v_buffer` -- pointers into a single contiguous allocation (`buffer_alloc`). The pointers are offset by `border * stride + border` to allow negative indexing for border pixels.
- `y_buffer_bit_inc`, `u_buffer_bit_inc`, `v_buffer_bit_inc` -- separate bit-increment planes for split-mode 10-bit storage (where 8 MSBs are in the main buffer, 2 LSBs are packed 4 pixels per byte in the bit_inc buffer).
- `y_stride`, `u_stride`, `v_stride` -- row strides in pixels (not bytes); equal to `max_width + 2 * border`.
- `width`, `height` -- current active dimensions (may differ from `max_width`/`max_height` after resize).
- `border` -- padding in pixels on each side (luma units).
- `luma_size` -- `y_stride * (max_height + 2 * border)`.
- `chroma_size` -- `u_stride * ((max_height + ss_y + 2 * border) >> ss_y)`.
- `packed_flag` -- true when samples are stored as 16-bit (not split mode).
- `buffer_enable_mask` -- bitmask selecting which planes are allocated (Y=0x1, Cb=0x2, Cr=0x4).
- `bit_depth` -- EB_EIGHT_BIT, EB_TEN_BIT, EB_SIXTEEN_BIT, or EB_THIRTYTWO_BIT.
- `color_format` -- EB_YUV420, EB_YUV422, or EB_YUV444.

**10-bit storage modes:**
1. **Split mode** (`split_mode = true`): 8-bit samples in `y_buffer` etc., 2 LSBs compressed 4:1 in `y_buffer_bit_inc` etc. Stride for bit_inc is `y_stride / 4` for compressed representation.
2. **Packed mode** (`packed_flag = true`): 16-bit samples directly in `y_buffer` etc., with `bytes_per_pixel = 2`.

**Allocation formula:**
```
alloc_size = 0
for each enabled plane:
    alloc_size += plane_size * bytes_per_pixel
    if split_mode:
        alloc_size += bit_inc_size
```
A single `EB_MALLOC_ALIGNED_ARRAY` call allocates the entire frame. Plane pointers are then computed as offsets into `buffer_alloc` with appropriate border adjustments.

### Yv12BufferConfig

AV1 reference implementation buffer format. Used for loop filter, CDEF, restoration, and global motion. Created by linking from `EbPictureBufferDesc` via `svt_aom_link_eb_to_aom_buffer_desc()`.

Key difference from `EbPictureBufferDesc`: uses `crop_width`/`crop_height` (actual content dimensions) alongside `width`/`height` (aligned dimensions). For high-bit-depth, buffer pointers use `CONVERT_TO_BYTEPTR` macro encoding (pointer value divided by 2, then cast to uint8_t*).

### PictureParentControlSet (PPCS)

The long-lived per-picture structure, created at Resource Coordination and surviving until Packetization releases it. Contains:

**Identity & ordering:**
- `picture_number` -- display order index
- `decode_order` -- encoding/decode order index
- `temporal_layer_index` -- position in hierarchical GoP (0 = base layer)
- `slice_type` -- I_SLICE, B_SLICE, P_SLICE
- `hierarchical_levels` -- depth of the prediction structure
- `idr_flag`, `cra_flag`, `scene_change_flag`, `end_of_sequence_flag`

**Input picture pointers:**
- `enhanced_pic` -- the (possibly temporally filtered) input picture
- `enhanced_unscaled_pic` -- always the original input
- `chroma_downsampled_pic` -- 420-subsampled version if input is 422/444
- `non_tf_input` -- original pre-temporal-filter input (when TF is applied)
- `ds_pics` -- struct with `picture_ptr`, `quarter_picture_ptr`, `sixteenth_picture_ptr`

**Pre-analysis results:**
- `variance[b64_idx][pu_idx]` -- per-block variance at 8x8/16x16/32x32/64x64 levels
- `pic_avg_variance` -- picture-level average variance
- `picture_histogram[region_w][region_h][256]` -- luma histogram per spatial region
- `average_intensity_per_region[w][h]` -- average luma per region
- `avg_luma` -- overall average luma (or INVALID_LUMA if not computed)

**Reference management:**
- `av1_ref_signal` -- `Av1RpsNode` with `ref_dpb_index[7]`, `ref_poc_array[7]`, `refresh_frame_mask`
- `ref_pic_wrapper` -- wrapper for the reference picture object allocated at Picture Manager
- `pa_ref_pic_wrapper` -- PA reference (padded + downsampled input, for ME)
- `ref_pa_pic_ptr_array[2][4]` -- pointers to PA references for each list
- `ref_frame_map[8]` -- maps frame buffer indices to reference slots
- `released_pics[9]` / `released_pics_count` -- pictures this frame releases from the DPB

**Encode control flow:**
- `child_pcs` -- pointer to the short-lived `PictureControlSet` (set by Picture Manager)
- `enc_dec_ptr` -- pointer to `EncDecSet` (recon + quantized coeff buffers)
- `me_data_wrapper` / `pa_me_data` -- motion estimation results
- `enc_mode` -- encoder speed preset
- Frame header (`frm_hdr`), AV1 common state (`av1_cm`)

### PictureControlSet (PCS)

The short-lived child structure, allocated from a pool by Picture Manager and released by Packetization. Contains:

- `ppcs` / `ppcs_wrapper` -- back-pointer to parent
- `scs` -- sequence control set pointer
- `bitstream_ptr` -- bitstream output object
- `ref_pic_ptr_array[2][4]` -- resolved reference picture pointers (from `EbReferenceObject`)
- `sb_ptr_array` -- array of SuperBlock structures for all SBs
- Neighbor arrays for mode decision and encode pass (luma/chroma recon, DC sign level, partition context, txfm context) -- allocated per tile
- `mi_grid_base` / `mip` -- AV1 mode info grid
- `enc_dec_segment_ctrl` -- segment structures for parallel encode-decode
- `ec_info` -- per-tile entropy coding info
- `rst_info[3]` -- restoration unit info per plane
- Various search level controls (interpolation, chroma, CDEF, compound mode, etc.)

### EncDecSet

Holds reconstruction and coefficient buffers, pooled separately from PCS:

- `recon_pic` -- 8-bit reconstructed picture buffer
- `recon_pic_16bit` -- 16-bit reconstructed picture buffer
- `quantized_coeff[sb_index]` -- per-SB quantized coefficient buffers (32-bit)

### EbReferenceObject

Stored in the reference picture pool. Contains:

- `reference_picture` -- reconstructed reference frame
- `downscaled_reference_picture[sr_scale][resize_scale]` -- downscaled variants for super-resolution/resize
- `resize_mutex[sr][resize]` -- per-scale mutexes for thread-safe downscale-on-demand
- `frame_context` -- CDF context from this frame
- `global_motion[7]` -- global motion parameters
- `mvs` -- motion field motion vectors (for MFMV)
- Per-SB statistics: `sb_intra`, `sb_skip`, `sb_64x64_mvp`, `sb_me_64x64_dist`, `sb_me_8x8_cost_var`, `sb_min_sq_size`, `sb_max_sq_size`
- `film_grain_params`, `ref_cdef_strengths`, loop filter levels, distortion deviations

### EbPaReferenceObject

Pre-analysis reference, used for motion estimation:

- `input_padded_pic` -- padded luma input at full resolution
- `quarter_downsampled_picture_ptr` -- 1/4 resolution (2x decimation)
- `sixteenth_downsampled_picture_ptr` -- 1/16 resolution (4x decimation from 1/4)
- Downscaled variants for super-res/resize with per-scale mutexes
- `avg_luma` -- average luma of the picture

### ReferenceQueueEntry

Used by Picture Manager to track reference availability:

- `picture_number`, `decode_order`
- `reference_object_ptr` -- wrapper to `EbReferenceObject`
- `reference_available` -- set to true when reconstruction is complete
- `feedback_arrived` -- set when CDF update feedback arrives (for frame_end_cdf_update_mode)
- `frame_context_updated` -- set when the frame's CDF context has been written
- `refresh_frame_mask` -- which DPB slots this frame updates
- `dec_order_of_last_ref` -- decode order of the last frame that will use this as a reference
- `is_valid` -- whether this entry is in use

### NeighborArrayUnit

Stores previously coded data (reconstruction samples, mode info, coefficients) for causal neighbors during encoding:

- `left_array` -- data for the left column neighbor
- `top_array` -- data for the top row neighbor
- `top_left_array` -- data for the diagonal neighbor (indexed by `left_size + x - y`)
- `unit_size` -- bytes per element (1 for uint8, 2 for uint16, 4 for uint32)
- `granularity_normal` / `granularity_top_left` -- spatial resolution (1 = sample-level, 4 = PU-level)

Array sizes: `left_array_size = max_picture_height >> granularity_log2`, `top_array_size = max_picture_width >> granularity_log2`, `top_left_array_size = (width + height) >> granularity_top_left_log2`.

The top-left array uses a diagonal indexing scheme where `index = left_array_size + (x >> gran) - (y >> gran)`.

## Algorithms

### Picture Buffer Allocation

Three constructor variants exist:

1. **`svt_picture_buffer_desc_ctor`** -- General-purpose. Supports 8-bit (1 byte/pixel), 16-bit packed (2 bytes/pixel), and split mode (1 byte/pixel main + bit_inc planes). The luma stride is forced to be a multiple of 8 for 2-bit compression alignment.

2. **`svt_picture_buffer_desc_ctor_noy8b`** -- Same as above but does NOT allocate the Y 8-bit plane (only bit-increment and chroma). Used when the Y plane comes from another source.

3. **`svt_recon_picture_buffer_desc_ctor`** -- For reconstruction buffers. Always uses `bytes_per_pixel = 2` for >8-bit. Uses `EB_CALLOC_ALIGNED_ARRAY` (zero-initialized) instead of `EB_MALLOC_ALIGNED_ARRAY`.

All constructors compute the total allocation size, perform a single aligned allocation, then partition it into plane regions with border offsets.

**Buffer pointer formula:**
```
y_buffer = buffer_alloc + (border + y_stride * border) * bytes_per_pixel
u_buffer = buffer_alloc + assigned_space + ((border >> ss_x) + u_stride * (border >> ss_y)) * bytes_per_pixel
```

### Picture Padding

Two categories of padding:

1. **Input padding** (`pad_input_picture`): Extends the right edge by replicating the last column pixel, and the bottom edge by copying the last row. Applied to make dimensions multiples of `MIN_BLOCK_SIZE` (8).

2. **Reference padding** (`svt_aom_generate_padding`): Full border padding on all four sides. Horizontal padding uses `memset` to replicate edge pixels. Vertical padding copies entire rows (including horizontal padding).

For 10-bit compressed (2-bit packed) data, `svt_aom_generate_padding_compressed_10bit` handles the 4-pixels-per-byte layout, replicating the edge pixel value across all 2-bit positions within pad bytes.

16-bit padding (`svt_aom_generate_padding16_bit`) uses the same algorithm with `uint16_t` operations.

### Downsampling (Decimation)

The `svt_aom_downsample_2d_c` function performs low-pass filtered downsampling:

```
for each output pixel at (ox, oy):
    input position = (ox * step + step/2, oy * step + step/2)
    value = (input[y-1][x-1] + input[y-1][x] + input[y][x-1] + input[y][x] + 2) >> 2
```

This is a 2x2 box filter centered at `(step/2, step/2)` offset, producing a 0-phase downsampled image. Applied in cascade:
- **Quarter (1/4)**: `step = 2` from full resolution
- **Sixteenth (1/16)**: `step = 2` from quarter, or `step = 4` from full (when quarter is not available)

Each downsampled picture is then padded with `svt_aom_generate_padding` to fill its borders.

### Variance Computation

Performed per 64x64 block in `compute_b64_variance`, using a hierarchical mean/mean-squared approach:

1. **8x8 level**: Compute mean and mean-of-squares for each 8x8 block. Two precision modes:
   - `BLOCK_MEAN_PREC_FULL`: Process each 8x8 individually using `svt_compute_mean_8x8` and `svt_compute_mean_square_values_8x8`
   - `BLOCK_MEAN_PREC_SUB`: Process four 8x8 blocks at once using `svt_compute_interm_var_four8x8` (subsampled -- processes every other row)

2. **16x16, 32x32, 64x64**: Each level averages the four child means and means-of-squares from the level below (shift right by 2).

3. **Variance**: `var = mean_of_squares - mean * mean`, shifted right by `VARIANCE_PRECISION` (16 bits).

Results stored in `pcs->variance[b64_idx][pu_idx]` where `pu_idx` uses ME_TIER_ZERO constants.

### Histogram Computation

`calculate_histogram` builds a 256-bin luma histogram over a region, with configurable decimation step:

```
for each pixel at (x, y) stepping by decim_step in both dimensions:
    histogram[pixel_value]++
    sum += pixel_value
```

The picture is divided into `picture_analysis_number_of_regions_per_width * ..._height` spatial regions. Histograms are scaled by `4 * 4 * decim_step * decim_step` to normalize for the subsampling. Used for scene change detection in Picture Decision.

### 10-bit Pack/Unpack

- **Pack** (`svt_aom_compressed_pack_sb`): Combines 8-bit buffer + 2-bit compressed buffer into 16-bit output. The 2-bit buffer stores 4 pixels per byte.
- **Unpack** (`svt_aom_un_pack2d`): Splits 16-bit input into 8-bit + N-bit buffers. Uses SIMD-optimized path when `(width % 4 == 0) && (height % 2 == 0)`.
- **Pack 2D source** (`svt_aom_pack2d_src`): Same pattern for source pictures.

### Distortion Computation

Three variants of full distortion kernels:

1. **32-bit coefficients**: `svt_full_distortion_kernel32_bits_c` computes both residual distortion `SUM((coeff - recon_coeff)^2)` and prediction distortion `SUM(coeff^2)`.
2. **CBF-zero 32-bit**: `svt_full_distortion_kernel_cbf_zero32_bits_c` -- when no coefficients are coded, both distortions equal `SUM(coeff^2)`.
3. **16-bit pixels**: `svt_full_distortion_kernel16_bits_c` computes SSE between 16-bit input and prediction.

### Residual Computation

`svt_residual_kernel8bit_c` and `svt_residual_kernel16bit_c`:
```
residual[x] = (int16_t)input[x] - (int16_t)pred[x]
```
Output is always `int16_t` regardless of input bit depth.

## Key Functions

### Resource Coordination

- **`svt_aom_resource_coordination_kernel`** -- Single-threaded entry point. Receives input frames from the application. For each frame:
  1. Gets a PPCS from the parent PCS pool
  2. Copies input samples into encoder buffers (handles 8-bit and 10-bit split modes)
  3. Handles resolution changes, rate changes, frame rate changes via `EbPrivDataNode` linked list
  4. Calls `reset_pcs_av1` to initialize all frame header defaults
  5. Assigns film grain random seed
  6. Posts `ResourceCoordinationResults` to Picture Analysis
  7. Handles overlay picture creation for alt-ref frames

- **`speed_buffer_control`** -- Dynamically adjusts `enc_mode` based on input buffer fullness, using configurable thresholds (`SC_FRAMES_INTERVAL_T1/T2/T3`). Runs only when `speed_control_flag` is enabled.

### Picture Analysis

- **`svt_aom_picture_analysis_kernel`** -- Multi-threaded. For each picture:
  1. Pads input to multiple of min block size (`svt_aom_pad_input_pictures`)
  2. Applies film grain denoise if configured (`svt_aom_picture_pre_processing_operations`)
  3. Downsamples 422/444 chroma to 420 (`svt_aom_down_sample_chroma`)
  4. Creates 1/4 and 1/16 downsampled luma for HME (`svt_aom_downsample_filtering_input_picture`)
  5. Computes block variance and luma histograms (`svt_aom_gathering_picture_statistics`)
  6. Detects screen content if configured
  7. Posts `PictureAnalysisResults` to Picture Decision

- **`svt_aom_downsample_filtering_input_picture`** -- Creates quarter and sixteenth resolution pictures with cascaded 2x2 box filtering and border padding.

- **`svt_aom_gathering_picture_statistics`** -- Computes histograms (from 1/16 picture) and block variances (from padded full-res input).

### Picture Manager

- **`svt_aom_picture_manager_kernel`** -- Single-threaded. Processes `PictureDemuxResults` messages of several types:
  - **`EB_PIC_INPUT`**: Adds picture to input queue, creates reference queue entry for ref frames
  - **`EB_PIC_REFERENCE`**: Marks reference as available in the reference queue
  - **`EB_PIC_FEEDBACK`**: Marks CDF feedback as arrived for frame context update
  - **`EB_PIC_SUPERRES_INPUT`**: Re-initializes child PCS for superres recode

  After processing each message, iterates over all queued pictures and starts any that are ready (all references available, in decode order). For each started picture:
  1. Gets an empty `EbReferenceObject` from the reference pool
  2. Gets an empty `EncDecSet` (recon + coeff buffers) from the enc-dec pool
  3. Gets an empty child `PictureControlSet` from the PCS pool
  4. Links child PCS to parent PPCS
  5. Initializes tile groups and encode-decode segments
  6. Tracks decode order using a min-heap to find consecutive completed pictures
  7. Posts `RateControlTasks` to the rate control process

- **Decode order tracking**: Uses a min-heap (`started_pics_dec_order`) to track which pictures have been started. `consecutive_dec_order` advances when the next expected decode order arrives, and any immediately following orders are popped from the heap.

### Packetization

- **`svt_aom_packetization_kernel`** -- Single-threaded. Receives entropy coding results and:
  1. Handles superres recode loops (comparing RDCOST of different superres denominators)
  2. Pads reference pictures and stores per-SB statistics to `EbReferenceObject`
  3. Writes frame header to bitstream
  4. Places frame into packetization reorder queue at its display-order position
  5. Counts frames in the next Temporal Unit (TU): walks reorder queue until a displayable frame is found
  6. Assembles TU: prepends Temporal Delimiter (2 bytes), concatenates all frame bitstreams
  7. For show-existing frames: encodes the show-existing header separately
  8. Handles undisplayed frames by pushing them into a sorted queue
  9. Sends `PictureDemuxResults` with `EB_PIC_REFERENCE` to Picture Manager to register the new reference
  10. Sends `PictureDemuxResults` with `EB_PIC_FEEDBACK` for CDF update synchronization
  11. Releases reference pictures whose `dec_order_of_last_ref` has been exceeded
  12. At EOS: releases all DPB entries and reference queue entries

- **`release_references_eos`** -- Walks the PD DPB and reference picture list, releasing all remaining references. Posts semaphores to unblock any waiting Picture Manager.

### Neighbor Arrays

- **`svt_aom_neighbor_array_unit_ctor`** -- Allocates left, top, and top-left arrays based on picture dimensions, unit size, and granularity. The type mask controls which arrays are allocated.

- **`svt_aom_neighbor_array_unit_reset`** -- Fills all arrays with `0xFF` (invalid marker).

- **`svt_aom_neighbor_array_unit_sample_write`** -- Copies reconstruction samples from a coded block to the appropriate positions in left, top, and top-left arrays.

- **`svt_aom_update_recon_neighbor_array`** -- Specialized update: copies bottom row to top-left (forward diagonal) and right column to top-left (reverse diagonal, stepping -1 in the array).

- **Index computation**:
  - Left: `loc_y >> granularity_normal_log2`
  - Top: `loc_x >> granularity_normal_log2`
  - Top-left: `left_array_size + (loc_x >> gran_tl_log2) - (loc_y >> gran_tl_log2)`

### PCS Construction

- **`picture_control_set_ctor`** -- Allocates:
  - Input frame 16-bit buffer (for 10-bit pipeline)
  - Hash table for hash-based motion search
  - SuperBlock array (`sb_ptr_array`)
  - Per-SB arrays: `sb_intra`, `sb_skip`, `sb_64x64_mvp`, `b64_me_qindex`, `sb_min_sq_size`, `sb_max_sq_size`
  - Bitstream object
  - Per-tile entropy coding info, encode-decode segments
  - Neighbor arrays for MD (per NA_TOT_CNT depth) and encode pass (per tile)
  - Mode info grid (`mi_grid_base`, `mip`)
  - Restoration info with stripe boundaries
  - Rate estimation context

- **`recon_coef_ctor`** -- Allocates reconstruction pictures (8-bit and/or 16-bit) and per-SB quantized coefficient buffers (32-bit).

## Dependencies

### Upstream (consumed by picture management)

| Module | Data Consumed |
|--------|---------------|
| Application input | Raw frames via `EbBufferHeaderType`, resolution/rate change events via `EbPrivDataNode` |
| Sequence Control Set | Encoding parameters, resolution, bit depth, SB geometry |
| Prediction Structure | GoP structure, temporal layer assignments, reference lists |

### Downstream (produced for other modules)

| Module | Data Produced |
|--------|---------------|
| Motion Estimation | Padded input + 1/4 + 1/16 downsampled pictures, variance data |
| Picture Decision | Analysis results (histograms, variance, screen content flags) |
| Mode Decision | Child PCS with all neighbor arrays, reference picture pointers |
| Entropy Coding | Tile info, segment info, frame header parameters |
| Rate Control | Reference feedback, picture statistics |
| Application | Output packets via reorder queue |

### Internal dependencies between picture management modules

```
resource_coordination_process --> pic_analysis_process --> pd_process
                                                              |
                                                              v
packetization_process <-- [enc pipeline] <-- pic_manager_process
        |                                          ^
        +------ EB_PIC_REFERENCE/FEEDBACK ---------+
```

## SIMD Functions

The following functions in the picture management subsystem have SIMD-optimized implementations:

| Function | C Reference | SSE2 | AVX2 | NEON | NEON DotProd | Purpose |
|----------|-------------|------|------|------|--------------|---------|
| `svt_aom_downsample_2d` | `svt_aom_downsample_2d_c` | Yes | Yes | Yes | - | 2D downsampling with 2x2 box filter |
| `svt_compute_mean_8x8` | `svt_compute_mean_c` | Yes | - | Yes | - | Mean of 8x8 block |
| `svt_compute_mean_square_values_8x8` | `svt_compute_mean_squared_values_c` | Yes | - | Yes | - | Mean of squared values for 8x8 block |
| `svt_compute_interm_var_four8x8` | `svt_compute_interm_var_four8x8_c` | Yes | Yes | Yes | Yes | Mean and mean-squared for four adjacent 8x8 blocks |
| `svt_compute_sub_mean_8x8` | `svt_compute_sub_mean_8x8_c` | Yes | - | Yes | - | Subsampled (every-other-row) mean for 8x8 |
| `svt_full_distortion_kernel32_bits` | `svt_full_distortion_kernel32_bits_c` | - | Yes | Yes | - | 32-bit coefficient distortion |
| `svt_full_distortion_kernel_cbf_zero32_bits` | `svt_full_distortion_kernel_cbf_zero32_bits_c` | - | Yes | Yes | - | Zero-CBF distortion shortcut |
| `svt_residual_kernel8bit` | `svt_residual_kernel8bit_c` | Yes | Yes | Yes | - | 8-bit residual computation |
| `svt_residual_kernel16bit` | `svt_residual_kernel16bit_c` | - | - | Yes | - | 16-bit residual computation |
| `svt_aom_un_pack2d_16_bit_src_mul4` | C fallback | Yes | Yes | Yes | - | Unpack 16-bit to 8+N-bit (width multiple of 4) |
| `svt_pack2d_16_bit_src_mul4` | C fallback | Yes | Yes | Yes | - | Pack 8+N-bit to 16-bit (width multiple of 4) |
| `svt_compressed_packmsb` | C fallback | Yes | Yes | Yes | - | Pack 8-bit + 2-bit compressed to 16-bit |
| `svt_unpack_and_2bcompress` | C fallback | Yes | Yes | Yes | - | Unpack 16-bit to 8-bit + 2-bit compressed |
| `svt_convert_8bit_to_16bit` | C loop | Yes | Yes | Yes | - | Widen 8-bit samples to 16-bit |
| `svt_av1_copy_wxh_8bit` | C memcpy loop | Yes | Yes | Yes | - | Block copy, 8-bit |
| `svt_av1_copy_wxh_16bit` | C memcpy loop | Yes | Yes | Yes | - | Block copy, 16-bit |
