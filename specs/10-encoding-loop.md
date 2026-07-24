# Encoding Loop

## Overview

The encoding loop is the central pipeline stage in SVT-AV1 that takes each superblock (SB) from partitioning through mode decision, transform, quantization, and reconstruction. It runs as the `svt_aom_mode_decision_kernel` thread, consuming `EncDecTasks` from the mode decision configuration process and producing `EncDecResults` for the entropy coding process.

The encoding loop operates in two passes (PD_PASS_0 and PD_PASS_1) when multi-pass partition decision is enabled. PD_PASS_0 performs a lightweight partition search to predict the optimal depth structure. PD_PASS_1 then refines this prediction with full-accuracy mode decision. Three distinct complexity tiers exist for each pass: Regular, Light PD1 (LPD1), and Light PD0 (LPD0), each trading accuracy for speed.

Within each block, mode decision proceeds through up to four MD stages (MDS0 through MDS3) that progressively narrow candidates: MDS0 evaluates all candidates with fast cost, MDS1/MDS2 apply full-loop evaluation with increasing accuracy to a reduced set, and MDS3 performs the final evaluation with full RDO including chroma, TX search, and RDOQ.

## Source Files

| File | Lines | Role |
|---|---|---|
| `Source/Lib/Codec/product_coding_loop.c` | 10706 | Central mode decision orchestrator. Contains `md_encode_block`, all MD stages (MDS0-MDS3), partition search (`svt_aom_pick_partition`, `svt_aom_pick_partition_lpd0`, `svt_aom_pick_partition_lpd1`), fast loop cores, full loop cores, TX type search, TX partitioning, NIC pruning, NSQ skip heuristics, candidate generation. |
| `Source/Lib/Codec/enc_dec_process.c` | 3307 | Thread entry point (`svt_aom_mode_decision_kernel`). Manages the segment-based parallelism loop, CDF updates per SB, multi-pass PD orchestration (PD0 then PD1), recode loop, SB-based deblocking filter, PSNR/SSIM calculations, recon output. |
| `Source/Lib/Codec/enc_dec_process.h` | 87 | Declares `EncDecContext` struct and `svt_aom_mode_decision_kernel`. |
| `Source/Lib/Codec/coding_loop.c` | 1923 | Encode pass (EncDec). Contains `svt_aom_encode_sb` (recursive SB encoding), `encode_b` (single block encode), `av1_encode_loop` (residual + transform + quantize), `av1_encode_generate_recon` (inverse transform + reconstruct), `perform_intra_coding_loop`, `perform_inter_coding_loop`. |
| `Source/Lib/Codec/coding_loop.h` | 48 | Declares `svt_aom_pick_partition*`, `svt_aom_encode_sb`, `svt_aom_init_sb_data`. |
| `Source/Lib/Codec/full_loop.c` | 2459 | Quantization kernels (`svt_aom_quantize_b_c`, `svt_aom_highbd_quantize_b_c`, `svt_av1_quantize_fp_*`), combined quantize+inv-quantize (`svt_aom_quantize_inv_quantize`, `svt_aom_quantize_inv_quantize_light`), trellis optimization (`svt_av1_optimize_b`), inverse transform wrapper, chroma full loop (`svt_aom_full_loop_uv`, `svt_aom_full_loop_chroma_light_pd1`). |
| `Source/Lib/Codec/full_loop.h` | 48 | Declares chroma full loop, inverse transform recon wrapper, `svt_aom_do_md_recon`. |
| `Source/Lib/Codec/enc_dec_results.c` | 30 | Constructor for `EncDecResults` output object. |
| `Source/Lib/Codec/enc_dec_results.h` | 59 | Declares `EncDecResults`, `DlfResults`, `CdefResults`, `RestResults` structs. |
| `Source/Lib/Codec/enc_dec_segments.c` | 148 | Segment initialization and dependency map construction for wavefront parallelism. |
| `Source/Lib/Codec/enc_dec_segments.h` | 87 | Declares `EncDecSegments`, `EncDecSegDependencyMap`, `EncDecSegSegmentRow` structs. Defines segment macros (`BAND_INDEX`, `ROW_INDEX`, `SEGMENT_INDEX`). |
| `Source/Lib/Codec/enc_dec_tasks.h` | 49 | Declares `EncDecTasks` struct. Defines task input types: `ENCDEC_TASKS_MDC_INPUT`, `ENCDEC_TASKS_ENCDEC_INPUT`, `ENCDEC_TASKS_CONTINUE`, `ENCDEC_TASKS_SUPERRES_INPUT`. |
| `Source/Lib/Codec/segmentation.c` | 311 | AV1 segmentation map management and QP-based segmentation application. |

## Test Coverage

| Test File | What It Tests |
|---|---|
| `test/QuantAsmTest.cc` | SIMD vs. C correctness for quantization kernels (`svt_aom_quantize_b`, `svt_av1_quantize_fp`, etc.) |
| `test/ResidualTest.cc` | SIMD vs. C correctness for `svt_residual_kernel8bit`/`svt_residual_kernel16bit` |
| `test/SpatialFullDistortionTest.cc` | SIMD vs. C for spatial distortion computation |
| `test/ForwardtransformTests.cc` | Forward transform correctness |
| `test/InvTxfm2dAsmTest.cc` | Inverse transform SIMD correctness |
| `test/EncodeTxbAsmTest.cc` | TXB encoding SIMD correctness |
| `test/SadTest.cc` | SAD computation (used in MDS0 fast cost) |
| `test/BlockErrorTest.cc` | Block error / distortion computation |

No dedicated unit tests exist for the high-level control flow (`svt_aom_mode_decision_kernel`, `svt_aom_pick_partition`, `md_encode_block`). These are tested implicitly through integration/conformance tests.

## Data Structures

### EncDecContext

The per-thread state for the encoding loop. Each EncDec worker thread owns one instance.

```
EncDecContext {
    mode_decision_input_fifo_ptr   // FIFO: receives EncDecTasks
    enc_dec_output_fifo_ptr        // FIFO: sends EncDecResults
    enc_dec_feedback_fifo_ptr      // FIFO: sends feedback tasks (for wavefront)
    md_ctx: ModeDecisionContext*   // Mode decision working state
    input_sample16bit_buffer       // 16-bit input buffer for HBD pipeline
    pic_fast_lambda[2]             // Lambda for 8-bit and 10-bit fast cost
    pic_full_lambda[2]             // Lambda for 8-bit and 10-bit full cost
    blk_ptr: BlkStruct*            // Current block being processed
    blk_org_x, blk_org_y          // Current block origin in picture coordinates
    sb_index                       // Current superblock index
    tile_group_index               // Current tile group
    tile_index                     // Current tile within the tile group
    coded_area_sb                  // Accumulated luma coded area within current SB
    coded_area_sb_uv               // Accumulated chroma coded area within current SB
    tot_intra_coded_area           // Running total of intra-coded area (for stats)
    tot_skip_coded_area            // Running total of skip-coded area
    coded_sb_count                 // Number of SBs processed by this thread
}
```

### EncDecTasks

Input message to the encoding loop thread, specifying what work to do.

```
EncDecTasks {
    pcs_wrapper: EbObjectWrapper*    // Picture control set
    input_type: uint32_t             // ENCDEC_TASKS_MDC_INPUT (start new tile group)
                                     // ENCDEC_TASKS_ENCDEC_INPUT (start specific row)
                                     // ENCDEC_TASKS_CONTINUE (continue wavefront)
                                     // ENCDEC_TASKS_SUPERRES_INPUT (superres recode)
    enc_dec_segment_row: int16_t     // Row index for ENCDEC_INPUT
    tile_group_index: uint16_t       // Which tile group to process
}
```

### EncDecSegments

Controls wavefront parallelism by dividing the picture into segments (bands of SBs along anti-diagonals).

```
EncDecSegments {
    dep_map: EncDecSegDependencyMap  // Per-segment dependency counters
    row_array: EncDecSegSegmentRow*  // Per-row segment tracking
    x_start_array, y_start_array    // Starting SB coordinates per segment
    valid_sb_count_array             // Number of valid SBs per segment
    segment_band_count               // Number of anti-diagonal bands
    segment_row_count                // Number of segment rows
    segment_ttl_count                // Total segments = row_count * band_count
}
```

### PC_TREE (Partition Coding Tree)

Recursive tree structure representing the SB partition hierarchy. Each node stores per-shape block data and links to four child split nodes.

```
PC_TREE {
    bsize: BlockSize                                // Block size at this depth
    mi_row, mi_col: int                             // MI-unit position
    partition: PartitionType                         // Selected partition (NONE, HORZ, VERT, SPLIT, etc.)
    rdc: { rd_cost: int64_t, valid: bool }          // RD cost of best partition at this depth
    block_data[NUM_SHAPES][MAX_NSQ_PER_SHAPE]       // BlkStruct pointers for each shape+sub-block
    tested_blk[NUM_SHAPES][MAX_NSQ_PER_SHAPE]       // Whether each sub-block has been tested
    split[4]: PC_TREE*                               // Children for PARTITION_SPLIT
    left_part_ctx, above_part_ctx                    // Partition context from neighbor arrays
}
```

### MdScan

Per-depth scan descriptor that specifies which shapes to test and whether to split further.

```
MdScan {
    mds_idx: uint32_t         // MDS (mode decision scan) index for the SQ block
    bsize: BlockSize          // Block size at this depth
    tot_shapes: uint32_t      // Number of shapes to test (0 = skip this depth)
    shapes[MAX_SHAPES]        // Array of shapes: PART_N, PART_H, PART_V, PART_HA, PART_HB, etc.
    split_flag: bool          // Whether to test splitting to next depth
    split[4]: MdScan*         // Children scan descriptors
    is_child: bool            // Whether this node was generated from a parent split
    index: uint8_t            // Depth index (0=128x128, 1=64x64, 2=32x32, 3=16x16, 4=8x8, 5=4x4)
}
```

### EncDecResults

Output message from the encoding loop to the next pipeline stage (deblocking filter / entropy coding).

```
EncDecResults {
    pcs_wrapper: EbObjectWrapper*   // Picture control set
}
```

## Algorithms

### 1. Main Encoding Loop Flow (svt_aom_mode_decision_kernel)

The encoding loop thread runs an infinite loop consuming `EncDecTasks` messages. For each message:

1. **Task dispatch**: Based on `input_type`:
   - `ENCDEC_TASKS_SUPERRES_INPUT`: Re-initialize for superres recode, post new MDC tasks for all tile groups, continue.
   - `ENCDEC_TASKS_MDC_INPUT` / `ENCDEC_TASKS_ENCDEC_INPUT` / `ENCDEC_TASKS_CONTINUE`: Enter the segment processing loop.
   - First-pass pictures (`svt_aom_is_pic_skipped`): Release ME data, post empty results, skip processing.

2. **CDF initialization**: If CDF update is enabled but per-SB update is disabled, estimate syntax/MV/coefficient rates once for the tile group using `pcs->md_frame_context`.

3. **Segment loop** (`assign_enc_dec_segments`):
   - The wavefront scheduler assigns segments based on dependency completion.
   - Each segment contains a contiguous range of SBs along an anti-diagonal band.
   - Processing proceeds: reset mode decision state, reset encode pass neighbor arrays.

4. **SB loop**: For each SB in the assigned segment:
   a. **CDF per-SB update**: If enabled, derive CDF context from weighted average of left (3x weight) and top-right (1x weight) neighbors. Re-estimate syntax, MV, and coefficient rates.
   b. **Configure SB**: Set QP, derive lambda, configure mode decision signals.
   c. **LPD0 classification**: Determine if Light PD0 can be used based on SB complexity.
   d. **PD_PASS_0** (if multi-pass PD enabled and not skipped):
      - Derive PD0 signals (light or regular).
      - Save neighbor arrays.
      - Call `svt_aom_pick_partition_lpd0` or `svt_aom_pick_partition` to determine predicted depth/partition.
      - Restore neighbor arrays.
      - Run LPD1 detector to classify SB complexity for PD1.
      - Perform predicted depth refinement (add/remove depths for PD1).
   e. **PD_PASS_1**:
      - Derive PD1 signals (light PD1 or regular).
      - Determine if partition structure is fixed (pred depth only + no NSQ).
      - Call `svt_aom_pick_partition_lpd1` or `svt_aom_pick_partition` for final partitioning.
   f. **Encode pass** (`svt_aom_encode_sb`): If EncDec is not bypassed, perform conformant encode/reconstruction for each block in the final partition tree.
   g. **SB-based deblocking**: If enabled, apply loop filter immediately per SB.

5. **Completion**: When all SBs are processed:
   - Accumulate intra/skip/HP statistics.
   - **Recode decision**: If VBR and recode is allowed, check if the frame should be re-encoded at a different QP. If so, re-initialize and re-post tasks.
   - Otherwise: release ME data, post `EncDecResults` to the next stage.

### 2. Block Partitioning: Recursive Partition Decision Tree

Three partition search functions handle different complexity tiers:

#### svt_aom_pick_partition (Regular)

Recursive function that searches the partition space at a given depth:

1. **Update partition neighbors**: Derive `left_part_ctx` and `above_part_ctx` from neighbor arrays for partition rate estimation.
2. **Test current depth** (`test_depth`):
   - Loop over all shapes set by `MdScan` (PART_N, PART_H, PART_V, PART_HA, PART_HB, PART_VA, PART_VB, PART_H4, PART_V4).
   - For each shape:
     a. Validate shape against picture boundaries (adjust block count if partially out of bounds).
     b. Compute partition rate cost via `svt_aom_partition_rate_cost`.
     c. For each sub-block in the shape:
        - Apply NSQ skip heuristics (`get_skip_processing_nsq_block`) for non-SQ shapes.
        - Call `md_encode_block` to perform full mode decision on the block.
        - Accumulate RD cost (partition rate + sum of block costs).
        - Early terminate if accumulated cost exceeds best known cost.
     d. If this shape has lower RD cost than the current best, update the PC_TREE partition.
3. **Skip sub-depth heuristic**: If `skip_sub_depth_ctrls` fires, disable splitting.
4. **Test split** (`test_split_partition`):
   - Compute SPLIT partition rate.
   - For each of the 4 quadrants, recursively call `svt_aom_pick_partition`.
   - Apply early exit: if parent cost times threshold is less than accumulated split cost, stop.
   - Compare total split cost to best current-depth cost. Select lower.
5. **Neighbor array update**: Update mode info and recon neighbor arrays for the winning partition.

#### svt_aom_pick_partition_lpd0 (Very Light PD0)

Simplified single-shape, luma-only, frequency-domain evaluation:

1. Only one shape is tested (typically PART_N; PART_H or PART_V at boundaries).
2. Call `md_encode_block_light_pd0` which:
   - Generates a small candidate set (ME + DC intra).
   - Evaluates using variance-based distortion (no transform).
   - Selects best candidate.
3. If `split_flag` is set, test split via `test_split_partition_lpd0` (recursive).
4. No partition rate is computed. No NSQ shapes.

#### svt_aom_pick_partition_lpd1 (Light PD1)

Fixed partition structure, 8-bit MD, single block per depth:

1. Only PART_N tested (or PART_H/PART_V at boundaries).
2. Call `md_encode_block_light_pd1` which runs a lightweight MDS0 + MDS3 pipeline.
3. Partition is fixed (no inter-depth comparison). Always update partition context.
4. If `split_flag` is set, recursively split via `test_split_partition_lpd1`.

### 3. Mode Decision Invocation per Block

Three block-level mode decision functions correspond to the three complexity tiers:

#### md_encode_block (Regular PD1)

Full-featured mode decision for a single block:

1. **Setup**: Apply segmentation QP, tune lambda (block-level or SSIM-based), copy reference frame types.
2. **ME data**: Derive ME offsets, optionally refine ME MVs with sub-pixel search.
3. **Reference pruning**: `perform_md_reference_pruning` reduces reference frame candidates based on ME costs.
4. **Predictive ME (PME)**: `pme_search` performs motion search around the best MVP.
5. **Independent UV search**: Optionally search for the best intra chroma mode before MDS0.
6. **Candidate generation**: `generate_md_stage_0_cand` builds the full candidate list (inter modes from ME, PME, global motion; intra modes including angular, palette, filter-intra; compound modes; intra-BC).
7. **NIC derivation**: `set_md_stage_counts` determines how many candidates survive each stage.
8. **MDS0**: For each candidate class, run `md_stage_0` (fast loop). Sort by fast cost, prune with `post_mds0_nic_pruning`.
9. **MDS1**: Run `md_stage_1` (first full loop) on surviving candidates. Sort by full cost, prune with `post_mds1_nic_pruning`.
10. **MDS2**: Run `md_stage_2` (second full loop with TXT for intra, IFS). Sort, prune with `post_mds2_nic_pruning`.
11. **MDS3**: Run `md_stage_3` (final full loop with TXS, RDOQ, chroma, spatial SSE). Call `svt_aom_product_full_mode_decision` to select the winner.
12. **Recon**: If needed, perform inverse transform to generate reconstructed samples.
13. **Copy recon**: Store reconstructed neighbor samples for future blocks' intra prediction.

#### md_encode_block_light_pd1

Reduced pipeline: MDS0 + MDS3 only, luma-only at MDS0, 8-bit, no NSQ, no PME, simplified ME refinement.

#### md_encode_block_light_pd0

Minimal pipeline: variance-based fast cost at MDS0, optional MDS3 for SSD refinement, no chroma, no transform type search.

### 4. The Encode-Decode Cycle

Each block undergoes the following signal processing chain during mode decision (in `full_loop_core`) and during the encode pass (in `av1_encode_loop` / `av1_encode_generate_recon`):

#### In Mode Decision (product_coding_loop.c)

```
for each candidate:
  1. PREDICT:
     - Intra: svt_av1_intra_prediction (angular, DC, smooth, paeth, CfL, palette, filter-intra)
     - Inter: svt_aom_inter_pu_prediction_av1 (motion compensation with interpolation filters,
              OBMC, warped motion, compound modes, inter-intra)

  2. RESIDUAL:
     - svt_aom_residual_kernel: src - pred -> residual (8-bit or 16-bit)

  3. TRANSFORM:
     - svt_aom_estimate_transform: forward DCT/ADST/identity
     - TX type search iterates over allowed TX types per block

  4. QUANTIZE:
     - svt_aom_quantize_inv_quantize (or _light variant): forward quantize + inverse quantize
     - Optional RDOQ (svt_av1_optimize_b): trellis-based coefficient optimization

  5. RATE ESTIMATION:
     - Coefficient bits estimated from quantized coefficients
     - Mode/reference/partition rate from entropy model

  6. DISTORTION:
     - SSD between source and reconstructed (or between source and prediction if no coefficients)
     - Optional spatial SSE for finer distortion measurement

  7. RD COST:
     - RDCOST(lambda, rate, distortion) = distortion + lambda * rate

  8. RECONSTRUCT (if needed for neighbor prediction):
     - svt_aom_inv_transform_recon_wrapper: inverse quantize -> inverse transform -> add to prediction
```

#### In Encode Pass (coding_loop.c)

When `bypass_encdec` is false, `encode_b` performs a conformant encode:

```
for each block in the final partition tree:
  1. If inter: perform_inter_coding_loop
     - Full motion compensation into recon buffer
     - For each TU: av1_encode_loop (residual, transform, quantize)
     - For each TU: av1_encode_generate_recon (inverse transform, add to pred)
     - Update neighbor arrays

  2. If intra: perform_intra_coding_loop
     - For each luma TU:
       - Copy neighbor arrays for intra prediction reference
       - svt_av1_predict_intra_block (conformant intra prediction)
       - av1_encode_loop (residual, transform, quantize)
       - av1_encode_generate_recon (inverse transform, add to pred)
       - Update recon neighbor arrays
     - For chroma TU:
       - Same cycle for Cb and Cr (with CfL if applicable)

  3. update_b: copy mode info, update CDF, copy to final SB block array
```

When `bypass_encdec` is true (common in higher-speed presets), the encode pass is skipped and the MD recon is used directly. The `update_b` function copies coefficients and recon from MD buffers to the EC buffers.

### 5. Segmentation Within the Loop

AV1 segmentation assigns segment IDs to blocks, which can modify QP and other coding parameters:

1. **Segmentation map reset**: At `segment_index == 0`, reset the `SegmentationNeighborMap`.
2. **Per-block application**: In `md_encode_block` and `md_encode_block_light_pd1`, if segmentation is enabled:
   - Call `svt_aom_apply_segmentation_based_quantization` to adjust `blk_ptr->qindex` based on the segment's `SEG_LVL_ALT_Q` feature.
3. **In the encode pass**: `av1_encode_loop` reads `seg_qp` from `segmentation_params.feature_data[segment_id][SEG_LVL_ALT_Q]` and passes it to the quantizer.

### 6. Tile and Segment-Based Parallelism

The encoding loop achieves parallelism at two levels:

#### Tile Group Parallelism

- The picture is divided into tile groups (`tile_group_cols * tile_group_rows`).
- Each tile group receives a separate `ENCDEC_TASKS_MDC_INPUT` task.
- Multiple threads can process different tile groups concurrently.

#### Wavefront Segment Parallelism (within a tile group)

- `EncDecSegments` divides each tile group into segments along anti-diagonal bands.
- Segments form a dependency graph: each segment depends on its left neighbor and bottom-left neighbor completing first.
- `assign_enc_dec_segments` manages the wavefront:
  1. `ENCDEC_TASKS_MDC_INPUT`: Reset all row indices, start segment 0 immediately.
  2. `ENCDEC_TASKS_CONTINUE`: After completing a segment:
     - Decrement dependency count of right neighbor.
     - Decrement dependency count of bottom-left neighbor.
     - If either neighbor's dependency reaches zero, self-assign or post a new task.
  3. Mutex-protected access to row assignment state ensures thread safety.

- Segment dimensions: `segment_band_count = row_count + col_count - 1`, `segment_row_count` configurable.
- Each segment maps to a range of SBs via `x_start_array`, `y_start_array`, and `valid_sb_count_array`.

### 7. The Full RDO Loop

The RDO (Rate-Distortion Optimization) loop in `md_encode_block` evaluates candidates through a multi-stage funnel:

#### Candidate Generation

`generate_md_stage_0_cand` creates candidates from:
- **Inter candidates**: ME results (uni-directional and bi-directional), PME results, NEAREST/NEAR/GLOBAL MVs, NEW_NEWMV compound, skip mode.
- **Intra candidates**: DC, smooth (H/V/HV), paeth, angular modes, filter intra, palette, intra-BC.
- **Compound candidates**: Inter-intra, compound with wedge/diffwtd masks.

Candidates are classified into classes (CAND_CLASS_0 through CAND_CLASS_TOTAL) for independent evaluation and pruning.

#### MD Stage 0 (MDS0) - Fast Loop

- **Light PD0**: Variance-based (SAE/SSE from ME variance functions). No transform. Single candidate may bypass even this.
- **Light PD1**: SATD/SAD-based distortion + approximate rate. Luma only, 8-bit.
- **Regular**: `fast_loop_core` computes:
  - Prediction (inter or intra).
  - Distortion: SAD, Hadamard (optional), or SSD.
  - Fast rate: approximate mode/reference/coefficient rate.
  - Fast cost = distortion + lambda * fast_rate.

Per-class sorting and NIC (Number of Intra Candidates) pruning reduce candidates. `post_mds0_nic_pruning` applies class-based and cross-class thresholds.

#### MD Stage 1 (MDS1) - First Full Loop

- `full_loop_core` with reduced features: no TXS, no TXT, no RDOQ, no chroma, no spatial SSE.
- Performs actual transform + quantize to get accurate luma coefficient information.
- `post_mds1_nic_pruning` further reduces candidates.

#### MD Stage 2 (MDS2) - Second Full Loop

- `full_loop_core` with TXT for intra candidates, optional IFS (Interpolation Filter Search).
- Inter candidates typically skip MDS2 if features are identical to MDS1.
- `post_mds2_nic_pruning` selects final candidates for MDS3.

#### MD Stage 3 (MDS3) - Final Full Loop

- `full_loop_core` with all features: TXS (TX depth search), TXT (TX type search), RDOQ, spatial SSE, chroma, CfL.
- For each surviving candidate:
  1. Inter prediction (with OBMC/WM refinement if applicable).
  2. Luma residual + transform + quantize.
  3. TX type search (`tx_type_search`): evaluates allowed TX types, selects best by RD cost.
  4. TX depth search (`perform_tx_partitioning`): evaluates TX depth 0, 1, 2.
  5. Chroma: residual + transform + quantize for Cb/Cr. CfL optimization if enabled.
  6. Full RD cost: `svt_aom_full_cost` combines luma distortion, chroma distortion, all rate components.
- `svt_aom_product_full_mode_decision` selects the winning candidate.

### 8. Early Termination Heuristics

Multiple heuristics reduce computation by skipping unnecessary work:

#### Partition-Level Early Termination

| Heuristic | Location | Description |
|---|---|---|
| **Split cost early exit** | `test_split_partition` | After each quadrant, if `parent_cost * threshold <= split_cost`, stop splitting. Separate thresholds for first quadrant (`split_cost_th`) and subsequent (`early_exit_th`). |
| **Skip sub-depth** | `eval_sub_depth_skip_cond1` | After testing current depth, skip further splitting based on SQ cost vs. recon-to-source deviation per quadrant. |
| **Variance-based skip** | `var_skip_sub_depth` | Skip sub-depths if source variance is very low (flat blocks). |
| **Depth removal** | `depth_removal_ctrls` | Entirely disallow depths below certain sizes (e.g., `disallow_below_64x64`, `disallow_below_32x32`). |
| **Pred depth only** | `pred_depth_only` | After PD0, only test the predicted depth at PD1 (no inter-depth competition). |

#### NSQ-Level Early Termination

| Heuristic | Function | Description |
|---|---|---|
| **NSQ split rate skip** | `update_skip_nsq_based_on_split_rate` | Skip NSQ shape if its partition rate cost is too high relative to SQ cost. |
| **SQ recon dist skip** | `update_skip_nsq_based_on_sq_recon_dist` | Skip NSQ if SQ reconstruction closely matches source in all quadrants. |
| **SQ TXS skip** | `update_skip_nsq_based_on_sq_txs` | Skip NSQ if SQ used TX depth 0 with no/few coefficients. |
| **H vs V rate** | `H_vs_V_split_rate_th` | Skip H partition if its rate is much higher than V (and vice versa). |
| **Redundant block** | `update_redundant` | Copy data from a previously coded identical block (e.g., the second block in PART_H when it matches a child from SPLIT). |

#### Candidate-Level Early Termination

| Heuristic | Description |
|---|---|
| **NIC pruning (post-MDS0/1/2)** | Thresholds relative to best cost eliminate weak candidates between stages. |
| **Reference pruning** | `perform_md_reference_pruning` eliminates reference frames with poor ME cost before candidate generation. |
| **Candidate elimination** | At MDS0, if an intra candidate's partial distortion exceeds threshold, skip it entirely. |
| **TX shortcut detector** | `tx_shortcut_detector` / `lpd1_tx_shortcut_detector`: if best MDS0/MDS1 distortion is very low relative to block area and QP, enable TX shortcuts at MDS3 (bypass TXT/TXS). |
| **Bypass TX** | In MDS3, if `bypass_tx_th` fires and block has no coefficients from MDS1, skip the entire transform. |
| **DCT-DCT only** | `search_dct_dct_only`: if conditions are met (low complexity, high speed), only test DCT-DCT transform type. |
| **Perform MDS1 bypass** | If only 1 candidate survives MDS0 pruning, skip MDS1 and go directly to MDS3. |

### 9. product_coding_loop: Role as Central Orchestrator

`product_coding_loop.c` is the largest file in the encoder (~10700 lines) and contains the complete mode decision logic for all three PD complexity tiers. Its role:

1. **Candidate generation**: `generate_md_stage_0_cand`, `generate_md_stage_0_cand_light_pd0`, `generate_md_stage_0_cand_light_pd1` build the candidate arrays.

2. **Fast loop evaluation**: `fast_loop_core`, `fast_loop_core_light_pd0`, `fast_loop_core_light_pd1` evaluate each candidate at MDS0.

3. **Full loop evaluation**: `full_loop_core`, `full_loop_core_light_pd0`, `full_loop_core_light_pd1` perform transform/quantize/RD for candidates at MDS1-MDS3.

4. **TX search**: `tx_type_search` evaluates transform types. `perform_tx_partitioning` evaluates TX depth splits. `perform_dct_dct_tx` and `perform_dct_dct_tx_light_pd1` handle the DCT-DCT-only fast path.

5. **Chroma**: `cfl_prediction`, `search_best_independent_uv_mode`, `check_best_indepedant_cfl` handle CfL alpha search and independent chroma mode evaluation.

6. **Motion search**: `md_sq_motion_search`, `md_nsq_motion_search`, `pme_search`, `read_refine_me_mvs`, `md_full_pel_search` perform various motion estimation refinement stages.

7. **NIC management**: `set_md_stage_counts`, `post_mds0_nic_pruning`, `post_mds1_nic_pruning`, `post_mds2_nic_pruning` control the candidate funnel.

8. **Partition search**: `svt_aom_pick_partition`, `svt_aom_pick_partition_lpd0`, `svt_aom_pick_partition_lpd1`, `test_depth`, `test_split_partition` implement the recursive partition decision tree.

9. **Block encoding orchestration**: `md_encode_block`, `md_encode_block_light_pd0`, `md_encode_block_light_pd1` tie everything together for a single block.

10. **Neighbor array management**: `mode_decision_update_neighbor_arrays`, `md_update_all_neighbour_arrays`, `svt_aom_copy_neighbour_arrays` maintain the spatial context needed for intra prediction and entropy context derivation.

## Key Functions

### Thread Entry Point

| Function | File | Description |
|---|---|---|
| `svt_aom_mode_decision_kernel` | enc_dec_process.c | Main thread loop. Consumes EncDecTasks, processes segments of SBs, produces EncDecResults. |

### Partition Search

| Function | File | Description |
|---|---|---|
| `svt_aom_pick_partition` | product_coding_loop.c | Regular recursive partition search with full NSQ + inter-depth RDO. |
| `svt_aom_pick_partition_lpd0` | product_coding_loop.c | Light PD0 partition search. Single shape, luma-only, variance distortion. |
| `svt_aom_pick_partition_lpd1` | product_coding_loop.c | Light PD1 partition search. Fixed partition, 8-bit, no NSQ. |
| `test_depth` | product_coding_loop.c | Evaluates all shapes at a given depth. Loops over shapes, calls `md_encode_block` per sub-block. |
| `test_split_partition` | product_coding_loop.c | Tests PARTITION_SPLIT by recursively calling `svt_aom_pick_partition` on quadrants. |
| `svt_aom_init_sb_data` | product_coding_loop.c | Initialize per-SB neighbor arrays and state before partition search. |

### Block-Level Mode Decision

| Function | File | Description |
|---|---|---|
| `md_encode_block` | product_coding_loop.c | Full mode decision pipeline (MDS0-MDS3) for a single block. |
| `md_encode_block_light_pd0` | product_coding_loop.c | Minimal mode decision for Light PD0. |
| `md_encode_block_light_pd1` | product_coding_loop.c | Reduced mode decision for Light PD1. MDS0 + MDS3 only. |

### MD Stages

| Function | File | Description |
|---|---|---|
| `md_stage_0` | product_coding_loop.c | MDS0: fast loop over candidates per class. |
| `md_stage_0_light_pd0` | product_coding_loop.c | MDS0 for Light PD0. |
| `md_stage_0_light_pd1` | product_coding_loop.c | MDS0 for Light PD1. |
| `md_stage_1` | product_coding_loop.c | MDS1: first full loop (no TXS/TXT/RDOQ/chroma). |
| `md_stage_2` | product_coding_loop.c | MDS2: second full loop (TXT for intra, IFS). |
| `md_stage_3` | product_coding_loop.c | MDS3: final full loop (TXS, TXT, RDOQ, chroma, spatial SSE). |
| `md_stage_3_light_pd0` | product_coding_loop.c | MDS3 for Light PD0. |
| `md_stage_3_light_pd1` | product_coding_loop.c | MDS3 for Light PD1. |

### Loop Core Functions

| Function | File | Description |
|---|---|---|
| `fast_loop_core` | product_coding_loop.c | MDS0 per-candidate evaluation: predict, distortion, fast rate, fast cost. |
| `fast_loop_core_light_pd0` | product_coding_loop.c | MDS0 for LPD0: variance-based cost, no transform. |
| `fast_loop_core_light_pd1` | product_coding_loop.c | MDS0 for LPD1: SAD/SATD + approximate rate. |
| `full_loop_core` | product_coding_loop.c | Full RDO per-candidate: predict, residual, TX search, quantize, distortion, rate, RD cost. |
| `full_loop_core_light_pd0` | product_coding_loop.c | Full loop for LPD0: single TX, subsampled residual. |
| `full_loop_core_light_pd1` | product_coding_loop.c | Full loop for LPD1: DCT-DCT, simplified TX. |

### TX Search

| Function | File | Description |
|---|---|---|
| `tx_type_search` | product_coding_loop.c | Iterates over allowed TX types, selects best by RD cost. |
| `perform_tx_partitioning` | product_coding_loop.c | Evaluates TX depth 0/1/2, selects best TX depth. |
| `perform_dct_dct_tx` | product_coding_loop.c | Fast path: only DCT-DCT TX type at depth 0. |
| `perform_dct_dct_tx_light_pd1` | product_coding_loop.c | DCT-DCT-only for LPD1. |

### Encode Pass

| Function | File | Description |
|---|---|---|
| `svt_aom_encode_sb` | coding_loop.c | Recursively walks the PC_TREE, calls `encode_b` for each leaf block. |
| `encode_b` | coding_loop.c | Encode a single block: dispatches to intra/inter coding loop, then `update_b`. |
| `av1_encode_loop` | coding_loop.c | Per-TU: compute residual, forward transform, quantize. For luma and chroma. |
| `av1_encode_generate_recon` | coding_loop.c | Per-TU: inverse transform, add to prediction to produce recon. |
| `perform_intra_coding_loop` | coding_loop.c | Intra encode: prediction per TU, encode loop, generate recon, update neighbors. |
| `perform_inter_coding_loop` | coding_loop.c | Inter encode: full-block prediction, encode loop per TU, generate recon. |
| `update_b` | coding_loop.c | Post-encode: accumulate stats, copy recon/coeffs, update CDF, copy to SB final array. |

### Quantization

| Function | File | Description |
|---|---|---|
| `svt_aom_quantize_inv_quantize` | full_loop.c | Combined forward quantize + inverse quantize. Dispatches to RDOQ or non-RDOQ path. |
| `svt_aom_quantize_inv_quantize_light` | full_loop.c | Lightweight quantize for PD0/PD1 paths. |
| `svt_aom_quantize_b_c` | full_loop.c | C reference: zbin-based quantization with QM support. |
| `svt_aom_highbd_quantize_b_c` | full_loop.c | C reference: high-bit-depth zbin quantization. |
| `svt_av1_quantize_fp_c` | full_loop.c | C reference: flat quantization (no zbin). |
| `svt_av1_optimize_b` | full_loop.c | Trellis-based RDOQ: optimizes quantized coefficients by evaluating rate-distortion tradeoffs. |

### Segmentation and Parallelism

| Function | File | Description |
|---|---|---|
| `assign_enc_dec_segments` | enc_dec_process.c | Wavefront segment scheduler. Returns true when a segment is available for processing. |
| `reset_enc_dec` | enc_dec_process.c | Reset per-segment state: lambda, neighbor arrays, segmentation map. |
| `svt_aom_enc_dec_segments_init` | enc_dec_segments.c | Initialize segment map: compute band/row indices, SB counts, dependency map. |
| `recode_loop_decision_maker` | enc_dec_process.c | Decide whether to re-encode the frame at a different QP (VBR recode). |

## Dependencies

The encoding loop depends on the following upstream modules:

| Module | Dependency |
|---|---|
| **Mode Decision Configuration** | Provides `EncDecTasks`, depth/block configuration (`MdScan`), initial QP. |
| **Motion Estimation** | ME results (`MeSbResults`) for inter candidate MVs. |
| **Rate Estimation** | `MdRateEstimationContext` for entropy-model-based rate estimates. |
| **Prediction** | `svt_av1_intra_prediction`, `svt_aom_inter_pu_prediction_av1` for sample generation. |
| **Transforms** | `svt_aom_estimate_transform` (forward), `svt_aom_inv_transform_recon_wrapper` (inverse). |
| **Entropy Coding Tables** | CDF arrays (`FRAME_CONTEXT`), scan orders, partition context lookup tables. |
| **Reference Pictures** | Reference picture buffers for inter prediction. |
| **Neighbor Arrays** | Spatial context arrays for intra prediction, partition context, DC sign, TX context. |

The encoding loop produces output consumed by:

| Module | Output |
|---|---|
| **Deblocking Filter** | Reconstructed samples, mode info per block. |
| **CDEF** | Reconstructed samples after deblocking. |
| **Restoration** | Reconstructed samples after CDEF. |
| **Entropy Coding** | Final block array (`sb_ptr->final_blk_arr`), quantized coefficients, mode info. |
| **Reference Picture Management** | Reconstructed picture for future frames' inter prediction. |

## SIMD Functions

The encoding loop relies on SIMD-accelerated primitives dispatched via RTCD (Runtime CPU Detection). Key functions with SIMD implementations:

### Quantization (ASM_AVX2, ASM_SSE4_1, ASM_NEON)

| RTCD Function Pointer | C Reference | SIMD Variants | Used In |
|---|---|---|---|
| `svt_aom_quantize_b` | `svt_aom_quantize_b_c` | SSE4.1, AVX2, NEON | `av1_quantize_b_facade_ii`, `svt_aom_quantize_inv_quantize` |
| `svt_aom_highbd_quantize_b` | `svt_aom_highbd_quantize_b_c` | SSE4.1, NEON | High-bit-depth quantization |
| `svt_av1_quantize_fp` | `svt_av1_quantize_fp_c` | AVX2, NEON | Flat quantization (no zbin) |
| `svt_av1_quantize_fp_32x32` | `svt_av1_quantize_fp_32x32_c` | AVX2, NEON | 32x32 TX flat quantization |
| `svt_av1_quantize_fp_64x64` | `svt_av1_quantize_fp_64x64_c` | AVX2, NEON | 64x64 TX flat quantization |
| `svt_av1_quantize_fp_qm` | `svt_av1_quantize_fp_qm_c` | - | Flat quantization with quant matrices |
| `svt_av1_highbd_quantize_fp` | `svt_av1_highbd_quantize_fp_c` | - | HBD flat quantization |

### Residual Computation (ASM_SSE2, ASM_SSE4_1, ASM_AVX2, ASM_AVX512, ASM_NEON)

| RTCD Function Pointer | C Reference | SIMD Variants | Used In |
|---|---|---|---|
| `svt_residual_kernel8bit` | C impl | SSE2, SSE4.1, AVX2, AVX512, NEON | `svt_aom_residual_kernel` (8-bit path) |
| `svt_residual_kernel16bit` | C impl | SSE2, SSE4.1, AVX2, AVX512, NEON | `svt_aom_residual_kernel` (16-bit path) |

### Distortion (ASM_SSE2, ASM_AVX2, ASM_AVX512, ASM_NEON)

| RTCD Function Pointer | Used In |
|---|---|
| `svt_spatial_full_distortion_kernel` | Spatial SSE computation in full loop |
| Variance functions (`svt_aom_mefn_ptr[bsize].vf`) | MDS0 fast cost in LPD0/LPD1 |
| SAD functions | MDS0 fast distortion for regular path |
| Hadamard transform | MDS0 Hadamard-based cost (optional) |

### Transform (ASM_SSE2, ASM_SSE4_1, ASM_AVX2, ASM_AVX512, ASM_NEON)

Forward and inverse transforms are SIMD-accelerated but are documented in the transforms specification. They are called from the encoding loop via:
- `svt_aom_estimate_transform` (forward)
- `svt_aom_inv_transform_recon_wrapper` (inverse + recon)
