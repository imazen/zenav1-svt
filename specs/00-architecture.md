# SVT-AV1 Encoder Architecture

## Overview

SVT-AV1 is a multi-threaded, pipeline-based AV1 encoder. The encoder decomposes the encoding process into a sequence of stages (called "processes" or "kernels"), each running as one or more threads. Data flows between stages via lock-free FIFO queues managed by a System Resource Manager (SRM). Each stage consumes input objects from an upstream queue, processes them, and posts results to a downstream queue.

The pipeline processes pictures in a combination of display order, decode order, and parallel order depending on the stage. Pictures are grouped into mini-GOPs (Groups of Pictures) whose size equals `2^hierarchical_levels`. Within a mini-GOP, pictures at different temporal layers are encoded in decode order, with parallelism possible across independent pictures.

## Source Files

| File | Path | Purpose |
|------|------|---------|
| enc_handle.h | Source/Lib/Globals/enc_handle.h | Central encoder handle: thread handles, contexts, system resources, buffer pools |
| enc_handle.c | Source/Lib/Globals/enc_handle.c | Encoder initialization, pipeline construction, thread creation, API implementations |
| sys_resource_manager.h | Source/Lib/Codec/sys_resource_manager.h | System Resource Manager: EbSystemResource, EbObjectWrapper, EbFifo, EbMuxingQueue |
| sys_resource_manager.c | Source/Lib/Codec/sys_resource_manager.c | SRM implementation: FIFO operations, object lifecycle, blocking/non-blocking get/post |
| resource_coordination_process.h | Source/Lib/Codec/resource_coordination_process.h | Stage 1: Resource Coordination declarations |
| resource_coordination_process.c | Source/Lib/Codec/resource_coordination_process.c | Stage 1: Input picture reception, PCS allocation, sequence parameter management |
| pic_analysis_process.h | Source/Lib/Codec/pic_analysis_process.h | Stage 2: Picture Analysis declarations |
| pic_analysis_process.c | Source/Lib/Codec/pic_analysis_process.c | Stage 2: Pre-analysis (variance, edge detection, noise estimation) |
| pd_process.h | Source/Lib/Codec/pd_process.h | Stage 3: Picture Decision declarations |
| pd_process.c | Source/Lib/Codec/pd_process.c | Stage 3: GOP structure, temporal layer assignment, reference picture selection |
| me_process.h | Source/Lib/Codec/me_process.h | Stage 4: Motion Estimation declarations |
| me_process.c | Source/Lib/Codec/me_process.c | Stage 4: Hierarchical motion estimation (HME + full-pel + sub-pel) |
| initial_rc_process.h | Source/Lib/Codec/initial_rc_process.h | Stage 5: Initial Rate Control declarations |
| initial_rc_process.c | Source/Lib/Codec/initial_rc_process.c | Stage 5: Initial QP assignment, TPL (Temporal Propagation Lookahead) preparation |
| src_ops_process.h | Source/Lib/Codec/src_ops_process.h | Stage 6: Source-Based Operations declarations |
| src_ops_process.c | Source/Lib/Codec/src_ops_process.c | Stage 6: TPL, temporal filtering, source-based statistics |
| pic_manager_process.h | Source/Lib/Codec/pic_manager_process.h | Stage 7: Picture Manager declarations |
| pic_manager_process.c | Source/Lib/Codec/pic_manager_process.c | Stage 7: Reference picture management, decode order scheduling, DPB management |
| rc_process.h | Source/Lib/Codec/rc_process.h | Stage 8: Rate Control declarations |
| rc_process.c | Source/Lib/Codec/rc_process.c | Stage 8: QP assignment per picture (CRF/CQP/VBR/CBR) |
| md_config_process.h | Source/Lib/Codec/md_config_process.h | Stage 9: Mode Decision Configuration declarations |
| md_config_process.c | Source/Lib/Codec/md_config_process.c | Stage 9: SB-level segmentation, mode decision parameter setup |
| md_process.h | Source/Lib/Codec/md_process.h | Stage 10: Mode Decision / EncDec declarations |
| md_process.c | Source/Lib/Codec/md_process.c | Stage 10: Per-SB mode decision (prediction mode search, transform, quantization) |
| enc_dec_process.h | Source/Lib/Codec/enc_dec_process.h | Stage 10 (continued): Encode-Decode process support |
| enc_dec_process.c | Source/Lib/Codec/enc_dec_process.c | Stage 10 (continued): Reconstruction, coefficient coding |
| dlf_process.h | Source/Lib/Codec/dlf_process.h | Stage 11: Deblocking Loop Filter declarations |
| dlf_process.c | Source/Lib/Codec/dlf_process.c | Stage 11: Deblocking filter application |
| cdef_process.h | Source/Lib/Codec/cdef_process.h | Stage 12: CDEF declarations |
| cdef_process.c | Source/Lib/Codec/cdef_process.c | Stage 12: Constrained Directional Enhancement Filter |
| rest_process.h | Source/Lib/Codec/rest_process.h | Stage 13: Restoration Filter declarations |
| rest_process.c | Source/Lib/Codec/rest_process.c | Stage 13: Loop restoration (Wiener, self-guided) filter |
| ec_process.h | Source/Lib/Codec/ec_process.h | Stage 14: Entropy Coding declarations |
| ec_process.c | Source/Lib/Codec/ec_process.c | Stage 14: CABAC bitstream generation per tile |
| packetization_process.h | Source/Lib/Codec/packetization_process.h | Stage 15: Packetization declarations |
| packetization_process.c | Source/Lib/Codec/packetization_process.c | Stage 15: OBU framing, output buffer assembly, reordering to display order |
| pcs.h | Source/Lib/Codec/pcs.h | PictureControlSet and PictureParentControlSet definitions |
| pcs.c | Source/Lib/Codec/pcs.c | PCS constructors and destructors |
| sequence_control_set.h | Source/Lib/Codec/sequence_control_set.h | SequenceControlSet: sequence-level parameters, thread counts, buffer counts |
| sequence_control_set.c | Source/Lib/Codec/sequence_control_set.c | SCS constructors and parameter derivation |
| encode_context.h | Source/Lib/Codec/encode_context.h | EncodeContext: shared mutable state across all pictures |
| encode_context.c | Source/Lib/Codec/encode_context.c | EncodeContext constructor |
| pred_structure.h | Source/Lib/Codec/pred_structure.h | Prediction structure definitions (GOP templates) |
| pred_structure.c | Source/Lib/Codec/pred_structure.c | GOP structure tables: flat, 2-level through 6-level hierarchical |
| definitions.h | Source/Lib/Codec/definitions.h | Core constants, enums, block sizes, transform sizes, macros |
| object.h | Source/Lib/Codec/object.h | Object lifecycle macros: EB_NEW, EB_DELETE, EbDctor pattern |
| svt_threads.h | Source/Lib/Codec/svt_threads.h | Thread, mutex, semaphore abstractions |
| svt_threads.c | Source/Lib/Codec/svt_threads.c | Platform-specific thread/sync implementations |
| enc_mode_config.h | Source/Lib/Codec/enc_mode_config.h | Preset-dependent feature configuration |
| enc_mode_config.c | Source/Lib/Codec/enc_mode_config.c | Maps enc_mode (preset) to algorithmic feature levels |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| test/api_test/SvtAv1EncApiTest.cc | API lifecycle: null pointer handling, init/deinit sequences |
| test/api_test/SvtAv1EncParamsTest.cc | Parameter validation: default, valid, and invalid values for each config parameter |
| test/api_test/params.h | Test vectors defining default/valid/invalid ranges for each encoder parameter |
| test/api_test/MultiEncoderTest.cc | Multiple encoder instances running concurrently |

## Data Structures

### EbEncHandle (the Encoder Instance)

Defined in `Source/Lib/Globals/enc_handle.h`. This is the central encoder object, allocated one per encoder instance. It owns:

- **Buffer pools** (EbSystemResource): PCS pool, parent PCS pool, ME pool, reference picture pools, input/output buffer pools
- **Thread handles**: One per pipeline stage (some stages have arrays for multi-threaded variants)
- **Thread contexts** (EbThreadContext): One per thread, containing stage-specific private data
- **Inter-stage queues** (EbSystemResource): One per connection between pipeline stages
- **Callbacks** (EbCallback): Error handler for fatal errors
- **FIFO pointers**: Producer/consumer FIFOs for input and output buffers

### EbSystemResource (System Resource Manager)

Defined in `Source/Lib/Codec/sys_resource_manager.h`. Manages a fixed-size pool of objects that flow through the pipeline. Contains:

- `object_total_count`: Number of pre-allocated objects in the pool
- `wrapper_ptr_pool`: Array of EbObjectWrapper pointers
- `empty_queue` (EbMuxingQueue): Queue of available (empty) object wrappers
- `full_queue` (EbMuxingQueue): Queue of completed (full) object wrappers

The SRM implements backpressure: if all objects are in use, `svt_get_empty_object()` blocks until one is released. Objects have a `live_count` for reference counting -- an object is returned to the empty queue only when `live_count` reaches 0 and `release_enable` is true.

### EbObjectWrapper

Wraps every object that flows through the pipeline. Fields:

- `object_ptr`: Pointer to the actual payload (e.g., PictureParentControlSet, ResultsObject)
- `live_count`: Reference count. Incremented when multiple consumers need the object; decremented on release. Object returns to pool when it reaches 0.
- `release_enable`: Flag to prevent premature release (used for SCS objects that must persist)
- `system_resource_ptr`: Back-pointer to owning EbSystemResource
- `next_ptr`: Linked-list pointer for FIFO implementation

### EbFifo

Single-linked-list FIFO with a counting semaphore for blocking and a mutex for thread safety:

- `counting_semaphore`: Blocks consumers until items are available
- `lockout_mutex`: Protects FIFO modifications
- `first_ptr` / `last_ptr`: Head/tail of the linked list
- `quit_signal`: Set to true during shutdown to unblock waiting threads
- `queue_ptr`: Back-pointer to parent EbMuxingQueue

### EbMuxingQueue

A fan-out dispatcher that assigns objects to waiting consumer FIFOs in round-robin fashion:

- `object_queue` (EbCircularBuffer): Pending objects waiting for a consumer
- `process_queue` (EbCircularBuffer): Waiting consumer FIFOs
- `process_fifo_ptr_array`: Array of per-consumer FIFOs

When an object is posted and a consumer is waiting, the object is immediately dispatched. Otherwise it queues until a consumer requests one.

### SequenceControlSet (SCS)

Defined in `Source/Lib/Codec/sequence_control_set.h`. Sequence-level parameters that rarely change:

- `static_config` (EbSvtAv1EncConfiguration): Copy of the user-provided configuration
- `seq_header` (SeqHeader): AV1 sequence header fields
- `enc_ctx` (EncodeContext): Shared mutable encoding state
- Resolution parameters: `max_input_luma_width`, `max_input_luma_height`, padding, subsampling
- Superblock geometry: `b64_size`, `sb_size`, `pic_width_in_b64`, `sb_total_count`
- Thread counts per stage: `picture_analysis_process_init_count`, `motion_estimation_process_init_count`, `enc_dec_process_init_count`, etc.
- Buffer pool sizes: `picture_control_set_pool_init_count`, `reference_picture_buffer_init_count`, etc.
- FIFO sizes: `resource_coordination_fifo_init_count`, etc.
- Feature flags: `mfmv_enabled`, `enable_dg` (dynamic GOP), `tpl`, `calculate_variance`

### PictureParentControlSet (PPCS)

Defined in `Source/Lib/Codec/pcs.h` at line 732. The primary per-picture data structure that flows through the first half of the pipeline (stages 1-8). Key fields:

- Picture identity: `picture_number`, `decode_order`, `temporal_layer_index`, `slice_type`
- GOP flags: `idr_flag`, `cra_flag`, `scene_change_flag`, `end_of_sequence_flag`
- Quantization: `picture_qp`, `tot_qindex`, `avg_qp`
- Buffers: `enhanced_pic` (input), `enhanced_downscaled_pic`, `chroma_downsampled_pic`
- Prediction structure: `pred_struct_ptr`, `pred_struct_index`
- Reference management: `ref_pic_wrapper`, `pa_ref_pic_wrapper`, reference order hints
- Wrappers: `input_pic_wrapper`, `y8b_wrapper`, `p_pcs_wrapper_ptr`, `scs_wrapper`
- AV1 common: `av1_cm` pointer to shared AV1 state (frame header, etc.)
- Tile info: `log2_tile_rows`, `log2_tile_cols`, `tile_group_info`

### PictureControlSet (PCS)

Defined in `Source/Lib/Codec/pcs.h` at line 162. The per-picture data structure for the second half of the pipeline (stages 9-15). Created as a child of PPCS. Key fields:

- `ppcs`: Back-pointer to parent PictureParentControlSet
- `scs`: Pointer to SequenceControlSet
- `bitstream_ptr`: Bitstream output buffer
- `ref_pic_ptr_array[2][4]`: Reference picture wrapper array (List 0, List 1; up to 4 refs each)
- `enc_dec_segment_ctrl`: Segment-level parallelism control for EncDec
- `ec_info`: Per-tile entropy coding state
- `entropy_coding_pic_mutex`: Synchronization for entropy coding completion
- Reconstruction buffers and CDEF/restoration state
- Per-picture coding statistics: `intra_coded_area`, `skip_coded_area`, `hp_coded_area`

### PredictionStructure

Defined in `Source/Lib/Codec/pred_structure.h`. Describes the temporal layering of a GOP:

- `pred_struct_entry_count`: Number of entries (== GOP size)
- `pred_struct_entry_ptr_array`: Array of PredictionStructureEntry
- `pred_type`: LOW_DELAY_P, LOW_DELAY_B, or RANDOM_ACCESS

Each PredictionStructureEntry contains:
- `temporal_layer_index`: Which temporal layer this picture belongs to (0 = base)
- `decode_order`: Position in decode order within the GOP

Pre-defined structures exist for 1 through 6 hierarchical levels (GOP sizes 1, 2, 4, 8, 16, 32).

### Av1RpsNode

Reference Picture Set node for AV1:

- `refresh_frame_mask`: 8-bit mask indicating which DPB slots to refresh
- `ref_dpb_index[7]`: DPB indices for LAST, LAST2, LAST3, GOLDEN, BWD, ALT2, ALT references
- `ref_poc_array[7]`: POC values for each reference

### EbCallback

Application callback structure for error reporting. Contains:
- `handle`: Opaque handle passed back to the application
- `error_handler`: Function pointer called on fatal encoder errors

## Pipeline Architecture

### Stage Order and Data Flow

The encoder pipeline consists of 15 stages connected by FIFO queues. Each stage runs as one or more OS threads executing a kernel function in an infinite loop:

```
Input (API) --> [1] Resource Coordination
                     |
                     v  (ResourceCoordinationResults)
                [2] Picture Analysis  (N threads)
                     |
                     v  (PictureAnalysisResults)
                [3] Picture Decision  (1 thread)
                     |
                     v  (PictureDecisionResults / ME tasks)
                [4] Motion Estimation  (N threads)
                     |
                     v  (MotionEstimationResults)
                [5] Initial Rate Control  (1 thread)
                     |
                     v  (InitialRateControlResults)
                [6] Source-Based Operations  (N threads)
                     |
                     v  (PictureDemuxResults -> Picture Manager input)
                     |   Also: TPL Dispenser (N threads, dispatched from here)
                     |
                [7] Picture Manager  (1 thread)
                     |
                     v  (RateControlTasks)
                [8] Rate Control  (1 thread)
                     |
                     v  (RateControlResults)
                [9] Mode Decision Configuration  (N threads)
                     |
                     v  (EncDecTasks)
               [10] EncDec / Mode Decision  (N threads)
                     |
                     v  (EncDecResults)
               [11] Deblocking Loop Filter  (N threads)
                     |
                     v  (DlfResults)
               [12] CDEF  (N threads)
                     |
                     v  (CdefResults)
               [13] Restoration Filter  (N threads)
                     |
                     v  (RestResults -> also feeds back to Picture Manager)
               [14] Entropy Coding  (N threads)
                     |
                     v  (EntropyCodingResults)
               [15] Packetization  (1 thread)
                     |
                     v
                Output (API)
```

### Stage Descriptions

**Stage 1: Resource Coordination** (1 thread: `svt_aom_resource_coordination_kernel`)
- Entry point for pictures into the pipeline
- Receives input commands from the API (via `input_cmd_resource_ptr`)
- Allocates a PictureParentControlSet from the pool
- Copies input picture data into internal buffers
- Handles sequence parameter changes, resolution changes
- Applies speed control (dynamic preset adjustment based on buffer fullness)
- Posts ResourceCoordinationResults downstream

**Stage 2: Picture Analysis** (N threads: `svt_aom_picture_analysis_kernel`)
- Computes pre-analysis statistics on the input picture
- Variance computation per block
- Edge detection
- Noise estimation
- Histogram computation
- Results used for scene change detection and adaptive decisions

**Stage 3: Picture Decision** (1 thread: `svt_aom_picture_decision_kernel`)
- Determines GOP structure and temporal layer assignment
- Scene change detection triggers key frames
- Assigns prediction structure (temporal layer, decode order)
- Sets up temporal filtering groups
- Dispatches motion estimation tasks for temporal filtering and normal ME
- Manages the mini-GOP window and dynamic GOP decisions

**Stage 4: Motion Estimation** (N threads: `svt_aom_motion_estimation_kernel`)
- Hierarchical Motion Estimation (HME): coarse-to-fine search
- Full-pel search refinement
- Sub-pel refinement (half-pel, quarter-pel, eighth-pel)
- Temporal filtering ME (shared infrastructure)
- Per-SB motion vectors stored for use in later stages

**Stage 5: Initial Rate Control** (1 thread: `svt_aom_initial_rate_control_kernel`)
- First-pass rate control computations
- TPL (Temporal Propagation Lookahead) setup
- Motion complexity analysis
- Initial QP derivation
- Lookahead window management

**Stage 6: Source-Based Operations** (N threads: `svt_aom_source_based_operations_kernel`)
- TPL dispenser coordination (dispatches TPL tasks to TPL worker threads)
- Temporal filtering execution
- Source-based statistics aggregation
- Posts results to Picture Manager via PictureDemuxResults

**TPL Dispenser** (N threads: `svt_aom_tpl_disp_kernel`)
- Worker threads for TPL (Temporal Propagation Lookahead) computation
- Dispatched by Source-Based Operations
- Computes per-block propagation costs for rate-distortion optimization

**Stage 7: Picture Manager** (1 thread: `svt_aom_picture_manager_kernel`)
- Central scheduling stage
- Manages the Decoded Picture Buffer (DPB)
- Releases pictures for encoding when all their references are available
- Enforces decode-order encoding constraints
- Allocates child PCS from parent PCS
- Manages reference picture lifecycle (allocation, reference counting, release)

**Stage 8: Rate Control** (1 thread: `svt_aom_rate_control_kernel`)
- Per-picture QP assignment
- CRF/CQP: QP from user setting + temporal layer offset
- VBR: Target bitrate-based QP adaptation with lookahead
- CBR: Buffer-model-based QP adaptation
- Recode loop decisions (whether to re-encode at different QP)
- Lambda derivation for RD optimization

**Stage 9: Mode Decision Configuration** (N threads: `svt_aom_mode_decision_configuration_kernel`)
- Per-picture setup before mode decision
- Segmentation map computation (adaptive quantization)
- SB-level delta-QP assignment
- Mode decision parameter initialization per SB
- Dispatches EncDec tasks (one per segment)

**Stage 10: EncDec / Mode Decision** (N threads: `svt_aom_mode_decision_kernel`)
- The core encoding stage; operates per-SB (superblock)
- Partition decision (recursive block splitting)
- Intra prediction mode search
- Inter prediction mode search (using ME results)
- Transform type selection
- Quantization and coefficient coding
- Rate-distortion optimization (RDO)
- Reconstruction of the encoded picture
- Multi-stage pipeline within MD: PD0 (fast screening) -> PD1 (refinement) -> PD2 (final)

**Stage 11: Deblocking Loop Filter** (N threads: `svt_aom_dlf_kernel`)
- Applies the AV1 deblocking filter to reconstructed frame edges
- Operates on block boundaries to reduce blocking artifacts
- Filter strength derived from QP and coding mode

**Stage 12: CDEF** (N threads: `svt_aom_cdef_kernel`)
- Constrained Directional Enhancement Filter
- Per-8x8-block directional analysis
- Strength search per 64x64 block
- Filter application to reduce ringing artifacts

**Stage 13: Restoration Filter** (N threads: `svt_aom_rest_kernel`)
- AV1 loop restoration filter
- Wiener filter: optimizes 7-tap separable filter per restoration unit
- Self-guided filter: edge-preserving filter with learned parameters
- Stripe-based processing with restoration unit granularity
- Also posts feedback to Picture Manager (reference picture ready signal)

**Stage 14: Entropy Coding** (N threads: `svt_aom_entropy_coding_kernel`)
- CABAC (Context-Adaptive Binary Arithmetic Coding) bitstream generation
- Per-tile entropy coding
- Writes all syntax elements: partition, prediction modes, motion vectors, coefficients, filter parameters
- Tile-level parallelism

**Stage 15: Packetization** (1 thread: `svt_aom_packetization_kernel`)
- Assembles OBU (Open Bitstream Unit) packets
- Writes sequence header, frame header, tile group OBUs
- Reorders output from decode order to display order
- Computes PSNR/SSIM if stat_report is enabled
- Posts completed packets to the output buffer for API retrieval
- Handles EOS (End of Stream) signaling

### Threading Model

Thread counts are determined at initialization based on `level_of_parallelism` (the `lp` parameter). The mapping from `lp` to per-stage thread counts is computed in `set_segments_numbers()` in enc_handle.c.

Single-threaded stages (always 1 thread):
- Resource Coordination
- Picture Decision
- Initial Rate Control
- Picture Manager
- Rate Control
- Packetization

Multi-threaded stages (thread count scales with `lp`):
- Picture Analysis
- Motion Estimation
- Source-Based Operations
- TPL Dispenser
- Mode Decision Configuration
- EncDec / Mode Decision (typically the largest thread pool)
- Deblocking Loop Filter
- CDEF
- Restoration Filter
- Entropy Coding

Within multi-threaded stages, work is distributed via segments. Each picture is divided into a grid of segments (e.g., `enc_dec_segment_col_count_array` x `enc_dec_segment_row_count_array`), and each thread processes one segment at a time.

### Object Lifecycle Pattern

1. Producer calls `svt_get_empty_object(producer_fifo)` -- blocks until a wrapper is available from the empty queue
2. Producer fills the wrapper's `object_ptr` with data
3. Producer calls `svt_post_full_object(wrapper)` -- posts to the full queue, waking a consumer
4. Consumer calls `svt_get_full_object(consumer_fifo)` -- blocks until a wrapper is available from the full queue (or returns NULL on shutdown)
5. Consumer processes the data
6. Consumer calls `svt_release_object(wrapper)` -- decrements `live_count`; if it reaches 0 and `release_enable` is true, the wrapper returns to the empty queue

For objects shared by multiple consumers, `svt_object_inc_live_count()` is called before posting, and each consumer calls `svt_release_object()` independently.

### Shutdown Protocol

1. API signals EOS by sending a buffer with `EB_BUFFERFLAG_EOS` flag
2. Resource Coordination propagates EOS through the pipeline
3. Each stage detects EOS and forwards it to the next stage
4. `svt_shutdown_process()` sets `quit_signal` on each consumer FIFO and posts the counting semaphore to unblock waiting threads
5. Threads receiving `EB_NoErrorFifoShutdown` from `svt_get_full_object()` exit their kernel loops

## Algorithms

### GOP Structure Assignment

1. Pictures arrive in display order at Picture Decision
2. The prediction structure is selected based on `hierarchical_levels` and `pred_structure` (RANDOM_ACCESS or LOW_DELAY)
3. For RANDOM_ACCESS with `hierarchical_levels=4`, the GOP size is 16, with temporal layers 0-4
4. Key frames (IDR/CRA) reset the GOP structure
5. Dynamic GOP (`enable_dg`) can modify the mini-GOP size based on scene complexity
6. Scene changes detected in Picture Analysis force key frame insertion

Supported hierarchical levels and GOP sizes:
| Hierarchical Levels | GOP Size | Temporal Layers |
|---------------------|----------|-----------------|
| 0 (flat) | 1 | 1 |
| 1 | 2 | 2 |
| 2 | 4 | 3 |
| 3 | 8 | 4 |
| 4 | 16 | 5 |
| 5 | 32 | 6 |

### Reference Picture Management

AV1 supports up to 8 reference frame slots (DPB) and 7 named references per frame: LAST, LAST2, LAST3, GOLDEN, BWD, ALT2, ALT.

The Picture Manager:
1. Maintains a DPB of reconstructed reference pictures
2. Assigns reference frames to each picture based on the prediction structure and MRP (Multi-Reference Prediction) controls
3. `refresh_frame_mask` determines which DPB slots are updated after encoding
4. Reference picture release is coordinated via `live_count` in EbObjectWrapper -- a reference picture stays alive as long as any future picture still needs it

### Encoding Preset System

Presets range from -1 (research) through 0 (highest quality) to 13 (fastest). The `enc_mode_config.c` file maps each preset to hundreds of algorithmic knobs:

- Partition search depth and complexity
- Number of intra/inter prediction candidates
- Motion estimation search area and refinement levels
- Transform type search breadth
- CDEF/restoration search aggressiveness
- Temporal filtering parameters
- TPL enable/disable
- Rate-distortion optimization precision

### Multi-Pass Encoding

SVT-AV1 supports single-pass and two-pass encoding:

- **Pass 1** (`ENC_FIRST_PASS`): Fast analysis pass that generates statistics (frame complexity, motion vectors, etc.) stored in `rc_stats_buffer`
- **Pass 2** (`ENC_SECOND_PASS`): Full encoding pass that uses pass-1 statistics for better rate control and quality decisions
- **Single pass** (`ENC_SINGLE_PASS`): Combined analysis and encoding in one pass

## Dependencies

- **Platform threading**: pthreads (POSIX) or Windows threads, abstracted through `svt_threads.h`
- **SIMD dispatch**: Runtime CPU feature detection selects optimized function pointers via `aom_dsp_rtcd.h` and `common_dsp_rtcd.h`
- **Memory management**: Custom allocator wrappers in `svt_malloc.h` with tracking for leak detection

## SIMD Functions

SVT-AV1 has extensive SIMD optimization across all encoding stages. The SIMD dispatch tables are generated at build time and resolved at runtime based on CPU capabilities. Key SIMD-optimized function categories:

**Motion Estimation:**
- SAD (Sum of Absolute Differences) computation: `compute_sad.h`
- `svt_sad_loop_kernel` and variants for different block sizes
- HME search kernels

**Transform/Quantization:**
- Forward transforms: `transforms.c` (DCT, ADST, identity)
- Inverse transforms: `inv_transforms.c`
- Quantization and dequantization

**Prediction:**
- Intra prediction: `intra_prediction.c` (DC, directional, smooth, paeth, CfL)
- Inter prediction: `inter_prediction.c` (motion compensation, compound prediction)
- Convolution filters: `convolve.c` (sub-pixel interpolation)
- Warped motion: `warped_motion.c`, `enc_warped_motion.c`

**In-Loop Filters:**
- Deblocking filter: `deblocking_filter.c`
- CDEF: `cdef.c`, `enc_cdef.c`
- Restoration: `restoration.c`
- Super resolution: `resize.c`

**Statistics/Analysis:**
- Variance computation: `pic_operators.c`
- Mean computation: `compute_mean.h`
- Noise estimation: `noise_model.c`, `noise_util.c`
- FFT: `fft.c`

**Pixel Operations:**
- Blending: `blend_a64_mask.c`
- Picture copy/pack/unpack: `pic_operators.c`
- PSNR/SSIM: `svt_psnr.c`

**Rate Estimation:**
- Cost computation: `rd_cost.c`
- Coefficient coding: `coefficients.c`, `full_loop.c`
- Entropy coding: `entropy_coding.c`

**Global Motion:**
- Feature detection: `corner_detect.c`, `corner_match.c`
- RANSAC: `ransac.c`
- Global motion estimation: `global_me.c`, `global_motion.c`

All SIMD variants (SSE2, SSSE3, SSE4.1, AVX2, AVX-512, NEON, SVE/SVE2) need portable scalar reimplementation for a language port. The C reference implementations exist in the same source files and serve as the fallback when no SIMD is available.
