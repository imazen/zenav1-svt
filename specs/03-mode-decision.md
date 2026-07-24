# Mode Decision

## Overview

Mode decision (MD) is the core encoder component that selects the best coding mode for each block in a superblock. It operates as a multi-stage pipeline that generates candidate coding modes (intra, inter, compound, palette, intraBC, etc.), evaluates them using rate-distortion optimization (RDO), and progressively prunes the candidate set through increasingly accurate (and expensive) evaluation stages.

SVT-AV1's MD system has three main operating paths, selected by the partitioning detector (PD):

- **Light PD0 (LPD0)**: Fastest path. Minimal candidates, no duplicate filtering, no sub-pel refinement. Used for easy blocks where simple partitioning suffices.
- **Light PD1 (LPD1)**: Intermediate path. More candidates than LPD0, includes duplicate filtering and basic MVP selection, but still uses simplified evaluation.
- **Full MD (PD1)**: Complete 4-stage pipeline (MDS0 through MDS3) with progressive pruning, full RDO, transform search, chroma search, and compound mode evaluation.

The MD pipeline processes one block at a time within a recursive partition search that tries different block sizes (from 128x128 down to 4x4) and selects the partition structure with the lowest RD cost.

## Source Files

| File | Lines | Role |
|------|------:|------|
| `product_coding_loop.c` | ~10700 | Main MD coding loop: MDS0-MDS3 stages, partition search (`pick_partition`, `pick_partition_lpd0`, `pick_partition_lpd1`), motion search, transform search, full-loop core |
| `mode_decision.c` | ~4700 | Candidate generation (`generate_md_stage_0_cand`), candidate injection (intra, inter, compound, palette, intraBC, global, PME), inter-intra search, WM/OBMC injection, full mode decision |
| `mode_decision.h` | ~220 | `ModeDecisionCandidate`, `ModeDecisionCandidateBuffer`, function pointer typedefs for prediction/cost functions |
| `md_process.c` | ~810 | `ModeDecisionContext` constructor and initialization, NIC allocation, lambda assignment, neighbor array reset |
| `md_process.h` | ~1200+ | `ModeDecisionContext` (main state struct), control structures for all MD features (TXT, RDOQ, sub-pel, PME, OBMC, inter-intra compound, NSQ, depth refinement, NIC pruning, etc.) |
| `md_config_process.c` | ~1200 | MD configuration kernel: quantizer construction, MFMV setup, intraBC context initialization |
| `md_config_process.h` | ~50 | `ModeDecisionConfigurationContext`, kernel entry point |
| `enc_mode_config.c` | ~13200 | Signal derivation for all speed presets: maps `EncMode` to control structure values for every MD feature |
| `enc_mode_config.h` | ~160 | Function declarations for signal derivation and feature level getters |
| `rd_cost.c` | ~1900 | Rate-distortion cost computation: `intra_fast_cost`, `inter_fast_cost`, `full_cost`, `full_cost_light_pd0`, coefficient rate estimation (`svt_av1_cost_coeffs_txb`), MV bit cost, partition rate cost |
| `rd_cost.h` | ~95 | RD cost function declarations, `RDCOST` macro, `av1_drl_ctx` |
| `md_rate_estimation.c` | ~1120 | Builds rate estimation tables from frame CDFs: `estimate_syntax_rate`, `estimate_coefficients_rate`, `estimate_mv_rate`, CDF update functions |
| `md_rate_estimation.h` | ~210 | `MdRateEstimationContext` (all syntax element rate tables), probability cost table |
| `coding_unit.h` | ~290 | `BlkStruct`, `EcBlkStruct`, `MacroBlockD`, `SuperBlock`, `TplStats` -- core block data structures |
| `full_loop.c` | (referenced) | Full-loop transform/quantization/reconstruction used by MDS1-MDS3 |

## Test Coverage

No dedicated unit tests for mode decision were found in the repository. MD is tested implicitly through encoder integration tests and conformance tests.

| Test Type | Location | Coverage |
|-----------|----------|----------|
| Integration | `test/` | Encoder-level tests exercise MD through full encode paths |
| Conformance | External | AV1 bitstream conformance validates MD output correctness |

## Data Structures

### ModeDecisionCandidate

Represents a single coding mode candidate to evaluate. Stored in `ctx->fast_cand_array`.

```
ModeDecisionCandidate:
    block_mi: BlockModeInfo       // prediction mode, ref frames, MVs, interp filters,
                                  // motion_mode, compound info, intra params
    pred_mv[2]: Mv                // predictor MV for rate estimation (unipred in [0])
    palette_info: PaletteInfo*    // palette color map and parameters
    wm_params_l0, wm_params_l1: WarpedMotionParams  // warp parameters per reference
    transform_type[MAX_TXB_COUNT]: TxType            // selected TX type per TU
    transform_type_uv: TxType     // chroma TX type
    palette_size[2]: uint8        // palette sizes for Y and UV
    cand_class: CandClass         // classification for NIC pruning
    skip_mode_allowed: bool       // whether skip_mode can be used
    drl_index: uint8              // dynamic reference list index
```

### ModeDecisionCandidateBuffer

Working buffer for evaluating a candidate. Contains prediction, residual, reconstructed, and quantized coefficient buffers, plus cost accumulators.

```
ModeDecisionCandidateBuffer:
    cand: ModeDecisionCandidate*     // pointer to the candidate being evaluated
    pred: EbPictureBufferDesc*       // prediction block
    rec_coeff: EbPictureBufferDesc*  // reconstructed coefficients
    residual: EbPictureBufferDesc*   // residual (shared temp buffer)
    quant: EbPictureBufferDesc*      // quantized coefficients
    recon: EbPictureBufferDesc*      // reconstructed block (shared temp buffer)
    fast_cost: uint64*               // MDS0 cost (ptr into cost array)
    full_cost: uint64*               // full RDO cost
    full_cost_ssim: uint64*          // SSIM-weighted cost (when SSIM tuning enabled)
    fast_luma_rate: uint64           // luma rate from fast cost
    fast_chroma_rate: uint64         // chroma rate from fast cost
    total_rate: uint64               // total bits
    luma_fast_dist: uint64           // luma distortion from MDS0
    full_dist: uint64                // full distortion
    cnt_nz_coeff: uint16             // count of non-zero coefficients
    quant_dc: QuantDcData            // DC quantization data per plane
    eob: EobData                     // end-of-block positions per TU per plane
    block_has_coeff: uint8           // whether block has any non-zero coefficients
    y_has_coeff: uint16              // per-TU luma coeff flags (bitmask)
    u_has_coeff, v_has_coeff: uint8  // chroma coeff flags
```

### Candidate Classes (CandClass)

Candidates are classified for NIC (number-of-intra-candidates/number-of-inter-candidates) scaling:

| Class | Description |
|-------|-------------|
| `CAND_CLASS_0` | Intra modes (non-palette, non-intraBC) |
| `CAND_CLASS_1` | Inter MVP modes (NEARESTMV, NEARMV, GLOBALMV, and compound variants) |
| `CAND_CLASS_2` | Inter MV-search modes (NEWMV, NEW_NEWMV) |
| `CAND_CLASS_3` | Palette modes |
| `CAND_CLASS_4` | IntraBC modes |

### MdRateEstimationContext

Pre-computed rate estimation tables derived from frame CDFs. One instance per frame, shared by all MD threads. Contains factored bit costs for every syntax element: partition, skip, prediction mode, reference frame, MV components, interpolation filter, motion mode, compound type, intra mode (KF and non-KF), angle delta, CFL alpha, palette, filter intra, transform type, coefficient levels, EOB, and restoration filters.

### ModeDecisionContext

The main per-thread state structure (~1200+ lines of fields in `md_process.h`). Key groups:

- **Control structures**: `inter_comp_ctrls`, `obmc_ctrls`, `txt_ctrls`, `rdoq_ctrls`, `md_pme_ctrls`, `md_sq_me_ctrls`, `md_subpel_me_ctrls`, `ref_pruning_ctrls`, `nic_ctrls`, `depth_refinement_ctrls`, `intra_ctrls`, `filter_intra_ctrls`, etc.
- **Candidate arrays**: `fast_cand_array`, `cand_bf_ptr_array`, `cand_bf_tx_depth_1/2`
- **Per-stage counts**: `md_stage_0_count[CAND_CLASS_TOTAL]`, `md_stage_1_count`, `md_stage_2_count`, `md_stage_3_count`
- **Block state**: `blk_ptr`, `blk_geom`, `blk_org_x/y`, `sb_origin_x/y`, `me_sb_addr`, `me_block_offset`
- **Lambda**: `full_lambda_md[2]`, `fast_lambda_md[2]` (indexed by bit depth: 8-bit vs 10-bit)
- **MV injection dedup**: `injected_mvs[][]`, `injected_ref_types[]`, `injected_mv_count`
- **Reference data**: `ref_mv_stack`, `inter_mode_ctx`, `ref_filtering_res`, `wm_sample_info`
- **Buffers**: `recon_coeff_ptr[TX_TYPES]`, `recon_ptr[TX_TYPES]`, `quant_coeff_ptr[TX_TYPES]`, `tx_coeffs`, CFL/OBMC/compound working buffers

### BlkStruct

Per-block output written by MD's `product_full_mode_decision`. Contains the winning mode, MVs, TX types, coefficient data, and warp parameters. Consumed by enc-dec and entropy coding.

## Algorithms

### 1. Mode Decision Pipeline (Full MD Path)

The full MD path for a single block proceeds through these steps:

#### Pre-MDS0 Preparation

1. **Lambda assignment** (`mode_decision_configure_sb`): Compute RD multipliers from QP. Separate fast (SAD-based) and full (SSD-based) lambdas for 8-bit and 10-bit paths. Lambda modulation based on temporal layer, intra percentage, TPL data, and SSIM tuning.

2. **Reference frame setup**: Copy reference frame type array, optionally reorder based on ME distortion (`determine_best_references`).

3. **MVP table generation** (`generate_av1_mvp_table`): Build reference MV stacks for all reference frame types. Required for NEWMV/NEARMV DRL index selection and MV rate estimation.

4. **ME MV read and refinement** (`read_refine_me_mvs`): Load ME MVs from PA stage, optionally refine to 1/8-pel via sub-pel search and/or SQ motion search.

5. **Reference pruning** (`perform_md_reference_pruning`): Limit which reference frames are tested per candidate group based on ME distortion deviation from best reference.

6. **Predictive ME (PME) search** (`pme_search`): Search around MVPs for each reference to find better MVs than ME provided. Full-pel search with optional sub-pel refinement.

7. **Independent chroma mode search** (if `CHROMA_MODE_0`): Find best UV mode independently before luma MD, to use during fast cost computation.

#### MDS0: Fast Candidate Evaluation (md_stage_0)

Evaluates all injected candidates with a low-cost metric:

- **Prediction**: Generate luma prediction (and chroma if available).
- **Distortion**: SAD or Hadamard (SATD) between source and prediction. Hadamard is more accurate but slower; controlled by `mds0_use_hadamard`.
- **Rate**: Approximate rate using pre-computed syntax element costs. Separate `intra_fast_cost` and `inter_fast_cost` functions. For inter modes, rate includes reference frame type, prediction mode, MV cost, interpolation filter, and compound type. For intra, rate includes intra mode, angle delta, filter intra, and palette.
- **Cost**: `RDCOST(lambda, rate, distortion) = (rate * lambda >> AV1_PROB_COST_SHIFT) + (distortion << RDDIV_BITS)`
- **Output**: Sorted candidates per class; top-N from each class advance to MDS1.

OBMC face-off may occur at MDS0: if enabled, the OBMC prediction is computed and compared against simple translation; if OBMC is worse, the OBMC candidate is dropped.

#### Post-MDS0 NIC Pruning (post_mds0_nic_pruning)

Class-level and candidate-level pruning:
- Candidates in classes whose best cost deviates from the overall best by more than `mds1_class_th` are eliminated.
- Within surviving classes, NIC counts are scaled based on band-based cost deviation.
- Inter candidate merging may combine CAND_CLASS_1 and CAND_CLASS_2 when ME/PME distortion is low.

#### MDS1: Refined Evaluation (md_stage_1)

Uses `full_loop_core` with progressively more accurate computation:
- Full prediction generation (inter/intra).
- DCT-DCT only transform + quantization (no TX type search yet).
- SSD-based distortion (optionally spatial SSE from MDS1 onward).
- More accurate rate estimation including coefficient rates.
- Interpolation filter search (IFS) may run at MDS1 if configured.

#### Post-MDS1 NIC Pruning (post_mds1_nic_pruning)

Further reduces candidate count using the same banding mechanism but with `mds2_class_th` and `mds2_cand_th` thresholds.

#### MDS2: Near-Final Evaluation (md_stage_2)

Same as MDS1 but with potentially different settings for:
- TX type search may be enabled.
- More accurate coefficient rate estimation.
- IFS may run at MDS2 if not already done.

#### Post-MDS2 NIC Pruning (post_mds2_nic_pruning)

Final pruning before the most expensive stage, using `mds3_class_th` and `mds3_cand_th`.

#### MDS3: Full RDO (md_stage_3)

The final and most expensive stage. For each surviving candidate:

1. **Full prediction**: Inter prediction with interpolation filters, OBMC, warped motion, compound modes.
2. **Transform type search** (`tx_type_search`): Test multiple TX types (e.g., DCT_DCT, ADST_DCT, etc.) per TU. Uses SATD pre-screening and rate-cost-based early exit.
3. **TX depth search** (`perform_tx_partitioning`): Test TX sizes from largest to split (e.g., 32x32 -> four 16x16). Compare RD cost at each depth.
4. **RDOQ**: Rate-distortion optimized quantization may adjust quantized coefficients.
5. **Chroma processing**: Full chroma prediction, transform, quantization, and cost.
6. **Independent UV search** (if `CHROMA_MODE_0` and last MDS): Search best UV mode among all UV prediction modes with full RDO.
7. **Full cost computation** (`svt_aom_full_cost`): Complete RD cost including all luma and chroma distortion and rate components.

#### Final Mode Decision (product_full_mode_decision)

Select the candidate with the lowest RD cost (or lowest SSIM-weighted cost when SSIM tuning is active). Write winning mode parameters to `BlkStruct`. If SSIM tuning is active, a two-pass selection first finds the SSD-best, then among candidates within a threshold of SSD-best, selects the SSIM-best.

### 2. Candidate Generation

Candidates are generated by `generate_md_stage_0_cand` (and its light PD0/PD1 variants). The injection order and candidate types depend on the operating path.

#### Intra Candidates

Injected by `inject_intra_candidates`:
- **DC_PRED**: Always injected.
- **Angular modes**: V_PRED, H_PRED, D45_PRED, D135_PRED, D113_PRED, D157_PRED, D203_PRED, D67_PRED. Subset controlled by `intra_mode_end` (speed preset dependent).
- **Smooth modes**: SMOOTH_PRED, SMOOTH_V_PRED, SMOOTH_H_PRED.
- **PAETH_PRED**: Predict from direction of smallest gradient.
- **Angle delta**: For directional modes, offsets of -3 to +3 from the nominal angle. Controlled by angular prediction level.
- **Filter intra** (`inject_filter_intra_candidates`): DC, V, H, D157, PAETH filter intra modes for blocks <= 32x32.
- **Palette** (`inject_palette_candidates`): K-means clustering of source colors. Up to `MAX_PAL_CAND` candidates with varying palette sizes (2..8).
- **IntraBC** (`inject_intra_bc_candidates`): Block copy from already-reconstructed region of current frame (screen content tool). Hash-based and exhaustive search.

Pruning: `dc_cand_only_flag` may be set based on ME distortion thresholds or variance analysis (`is_dc_only_safe`), skipping all modes except DC.

#### Inter Candidates

Injected by `svt_aom_inject_inter_candidates`:

1. **PA ME candidates** (`inject_new_candidates`): NEWMV/NEW_NEWMV from pre-analysis motion estimation results. Each unipred candidate may additionally spawn:
   - Inter-intra compound (if enabled and block size 8x8..32x32)
   - Warped motion (if enabled and warp samples available)
   - OBMC (if enabled and overlappable neighbors exist)

2. **3x3 unipred refinement** (`unipred_3x3_candidates_injection`): Refine ME MVs by +/-1 pel in 8 positions around the ME MV.

3. **3x3 bipred refinement** (`bipred_3x3_candidates_injection`): Combine List0 and List1 MVs with +/-1 pel offsets. Controlled by `Bipred3x3Controls`.

4. **MVP candidates** (`inject_mvp_candidates_ii`): NEARESTMV, NEARMV (with multiple DRL indices), NEAREST_NEARESTMV, NEAR_NEARMV, NEAREST_NEWMV, NEW_NEARESTMV, NEAR_NEWMV, NEW_NEARMV. Controlled by `NearCountCtrls`.

5. **Global motion** (`inject_global_candidates`): GLOBALMV and GLOBAL_GLOBALMV using frame-level warp parameters.

6. **PME candidates** (`inject_pme_candidates`): NEWMV/NEW_NEWMV from predictive ME search results.

7. **Compound modes** (`inj_comp_modes`): For bipred candidates, test compound types beyond average: COMPOUND_DISTWTD, COMPOUND_DIFFWTD, COMPOUND_WEDGE. Controlled by `InterCompCtrls`.

#### Duplicate Filtering

All injected MVs are checked against `ctx->injected_mvs[]` to avoid duplicate candidates. For bipred, a "redundant candidate" detector with configurable score threshold can also prune near-duplicate MVs.

### 3. Rate-Distortion Optimization

#### RD Cost Function

The fundamental cost metric used throughout MD:

```
RDCOST(rm, rate, distortion) =
    ROUND_POWER_OF_TWO(rate * rm, AV1_PROB_COST_SHIFT) + (distortion << RDDIV_BITS)
```

Where:
- `rm` = RD multiplier (lambda), derived from QP. Separate values for 8-bit (`full_lambda_md[0]`) and 10-bit (`full_lambda_md[1]`). 10-bit lambda is 16x the 8-bit lambda for full cost, 4x for fast cost.
- `rate` = estimated bits in "fac_bits" units (factored cost from CDF probabilities, scaled by `AV1_PROB_COST_SHIFT = 9`)
- `distortion` = pixel-domain distortion metric (SAD, SATD, SSD, or SSIM-weighted SSD depending on stage)
- `RDDIV_BITS = 7`

#### Fast Cost (MDS0)

Two functions compute MDS0 cost:

- **`svt_aom_intra_fast_cost`**: Computes rate from intra mode signaling (y-mode for KF/non-KF, angle delta, filter intra, palette header, intra/inter flag, skip_mode flag, chroma mode). Distortion is the luma SAD/SATD from prediction.

- **`svt_aom_inter_fast_cost`**: Computes rate from inter mode signaling (reference frame type, prediction mode, DRL index, MV difference, compound parameters, interpolation filter, motion mode, skip_mode flag). Uses `svt_av1_mv_bit_cost` for MV rate or `svt_av1_mv_bit_cost_light` (approximation: `1296 + 50 * (|dx| + |dy|)`).

#### Full Cost (MDS3)

**`svt_aom_full_cost`**: Combines luma and chroma distortion and rate. Distortion includes both SSD and reconstructed-vs-source SSD. Rate includes coefficient coding cost (`svt_av1_cost_coeffs_txb`), TX size bits, skip flag, and all mode signaling bits.

**`svt_aom_full_cost_light_pd0`**: Simplified version using approximate partition cost (context index 0) and coefficient-only rate.

#### Coefficient Rate Estimation

**`svt_av1_cost_coeffs_txb`**: Computes the rate cost of quantized transform coefficients for a single transform block. Steps:

1. TXB skip cost (from `txb_skip_ctx`).
2. Transform type rate (from TX set membership, intra/inter mode).
3. EOB position cost (from level-map EOB cost tables).
4. Per-coefficient cost: base cost, sign bit (for DC), level residual coding (base range + Golomb for large values).
5. Optimized inner loop (`av1_cost_coeffs_txb_loop_cost_eob`) processes coefficients from EOB-1 down to 0, with fast coefficient estimation level controlling how many mid-frequency coefficients are evaluated.

#### MV Bit Cost

**`svt_av1_mv_bit_cost`**: Full MV cost using NMV joint cost + component costs from pre-computed tables, weighted by `MV_COST_WEIGHT = 108`, right-shifted by 7.

**`svt_av1_mv_bit_cost_light`**: Approximate cost: `1296 + 50 * (|mv.x - ref.x| + |mv.y - ref.y|)`. Used in light PD paths and when `approx_inter_rate` is set.

#### Partition Rate Cost

**`svt_aom_partition_rate_cost`**: Computes the rate cost of signaling a partition type for a block, using the factored partition CDF costs indexed by left/above context.

### 4. Block Partitioning

SVT-AV1 uses three partition search strategies corresponding to the three PD levels:

#### Full Partition Search (pick_partition)

`svt_aom_pick_partition` in `product_coding_loop.c`:

Recursive top-down search from the SB root (64x64 or 128x128):

1. **Evaluate current depth** (`test_depth`): Run `md_encode_block` for the current block size (PARTITION_NONE).
2. **Test split** (`test_split_partition`): Recursively call `pick_partition` for 4 sub-blocks.
3. **Depth refinement** (`DepthRefinementCtrls`): Compare PARTITION_NONE cost vs. PARTITION_SPLIT cost to decide whether to prune parent or child depths:
   - If parent cost is much better than sub-block costs, skip further splitting.
   - If sub-block costs are much better, skip testing parent in future iterations.
   - Thresholds `parent_to_current_th` and `sub_to_current_th` control aggressiveness.
   - Cost-band-based modulation adjusts thresholds based on absolute cost level.
   - QP-based modulation adjusts thresholds based on quantization parameter.
   - Split rate threshold: skip child depth if PARTITION_SPLIT rate alone is too high relative to current cost.
4. **NSQ evaluation**: Within each depth, non-square partitions (PARTITION_HORZ, PARTITION_VERT, PARTITION_HORZ_A/B, PARTITION_VERT_A/B, PARTITION_HORZ_4, PARTITION_VERT_4) are tested if `nsq_geom_ctrls` allows them.
5. **NSQ pruning**: Multiple signals skip NSQ based on:
   - SQ TX-search data (`update_skip_nsq_based_on_sq_txs`)
   - SQ recon distortion distribution (`update_skip_nsq_based_on_sq_recon_dist`)
   - Split rate vs. NONE cost (`update_skip_nsq_based_on_split_rate`)
   - Redundancy with previously tested shapes

#### Light PD0 Partition Search (pick_partition_lpd0)

`svt_aom_pick_partition_lpd0`: Simplified partition search:

1. Always evaluates PARTITION_NONE at each depth using `md_encode_block_light_pd0`.
2. Split decision based on ME distortion, variance, and cost comparisons.
3. No NSQ partitions tested.
4. Depth range controlled by `DepthRemovalCtrls` which can disallow depths below certain sizes based on ME distortion and variance thresholds.

#### Light PD1 Partition Search (pick_partition_lpd1)

`svt_aom_pick_partition_lpd1`: Intermediate complexity:

1. Evaluates PARTITION_NONE using `md_encode_block_light_pd1`.
2. Split decision uses cost-based thresholds.
3. NSQ partitions may be tested depending on settings.
4. Depth refinement similar to full path but with simplified thresholds.

### 5. Speed Presets

SVT-AV1 supports presets from `ENC_MR` (maximum quality/research) through `ENC_M0`..`ENC_M13` (fastest). Signal derivation in `enc_mode_config.c` maps each preset to specific control values. Key speed-quality tradeoffs:

#### Candidate Reduction

| Feature | Slow Presets (M0-M3) | Medium (M4-M7) | Fast (M8-M13) |
|---------|---------------------|-----------------|----------------|
| Max candidates | ~1900 | ~800-1200 | ~200-500 |
| Inter candidate groups | All enabled | PME/3x3 reduced | Minimal groups |
| Unipred candidates | All ME results | Reduced if bipred available | Only best per list |
| Compound types | AVG+DIST+DIFF+WEDGE | AVG+DIST+DIFF | AVG only |
| Inter-intra | Enabled with wedge | Reduced | Off |
| OBMC | Large blocks | Medium blocks | Off |
| Warped motion | Full refinement | Reduced iterations | Off |
| Palette search | Full | Reduced | Off |
| Filter intra | Enabled | Reduced | Off |
| IntraBC | Full search | Reduced | Off or reduced |

#### NIC Scaling

The NIC scaling level (0-15) controls how many candidates survive each MD stage. The scaling numerators per stage are:

- Level 0: 20/16 (125%) -- allows expansion
- Level 5: 8/16 (50%)
- Level 10: 3/16 (~19%)
- Level 15: 0/16 (only 1 candidate per class)

Higher presets use higher NIC levels (more aggressive pruning).

#### MD Stage Bypass

- `bypass_md_stage_1`: When true, skip MDS1 entirely (go from MDS0 directly to MDS2 or MDS3).
- `bypass_md_stage_2`: When true, skip MDS2.
- `MD_STAGING_MODE_0`: Only MDS0 + MDS3 (stages 1 and 2 bypassed).
- `MD_STAGING_MODE_1`: MDS0 + MDS1 + MDS3 (stage 2 bypassed).
- `MD_STAGING_MODE_2`: All 4 stages active.

#### Depth Control

- `disallow_4x4`: Fastest presets disable 4x4 blocks entirely.
- `disallow_8x8`: Very fast presets on high-resolution content disable 8x8 blocks.
- `DepthRemovalCtrls`: ME-distortion-based and variance-based disabling of small block sizes per SB.
- `DepthRefinementCtrls`: Controls how aggressively parent/child depths are pruned based on cost deviation.

#### Motion Search

| Feature | Slow | Medium | Fast |
|---------|------|--------|------|
| ME search area | 96x96 to 256x256 | 24x12 to 48x32 | 8x1 to 16x9 |
| HME L0 area | 240x240 | 32x32 to 192x192 | 8x8 to 96x96 |
| MD SQ motion search | Full with sparse levels | Reduced area | Off |
| MD PME search | Full | Reduced | Off |
| Sub-pel refinement | 1/8-pel, 8-tap | 1/4-pel, 4-tap | 1/2-pel or full-pel |
| Sub-pel search method | SUBPEL_TREE | SUBPEL_TREE_PRUNED | Skipped for many blocks |

#### Transform and Quantization

| Feature | Slow | Medium | Fast |
|---------|------|--------|------|
| TX type search | Full group (up to 16 types) | Reduced groups | DCT_DCT only |
| TX depth search | Depth 0, 1, 2 | Depth 0, 1 | Depth 0 only |
| RDOQ | Full | Reduced (skip UV, DCT_DCT only) | Off |
| TXS cycle reduction | Conservative thresholds | Aggressive thresholds | Off (no TXS) |

### 6. Transform Type Selection

TX type selection occurs within MDS3 (or MDS1/MDS2 at lower quality) via `tx_type_search` and `perform_tx_partitioning`.

#### TX Type Groups

TX types are organized into groups of decreasing complexity, controlled by `TxtControls`:

- **Group 1**: All 16 types (DCT_DCT, ADST_DCT, DCT_ADST, ADST_ADST, FLIPADST_DCT, DCT_FLIPADST, FLIPADST_FLIPADST, ADST_FLIPADST, FLIPADST_ADST, IDTX, V_DCT, H_DCT, V_ADST, H_ADST, V_FLIPADST, H_FLIPADST)
- **Group 2-5**: Progressively smaller subsets
- **DCT_DCT only**: Fastest option

Separate group settings for inter vs. intra and for TX blocks < 16x16 vs. >= 16x16.

#### SATD-Based Early Exit

Before full transform + quantization for each TX type, compute SATD (Hadamard transform of residual). If the SATD for the current TX type deviates from the best SATD by more than `satd_early_exit_th_intra` or `satd_early_exit_th_inter`, skip this TX type.

#### Rate-Cost Early Exit

If the rate cost of signaling the current TX type exceeds `txt_rate_cost_th` percent of the best candidate's total cost, skip this TX type.

#### Distortion/Coefficient Early Exit

If the best TX type's distortion per pixel is below `early_exit_dist_th`, or if the best TX type's coefficient count is below `early_exit_coeff_th`, skip remaining TX types.

#### TX Depth Search

`perform_tx_partitioning` tests TX sizes at multiple depths:
- Depth 0: Largest TX that fits the block (e.g., 32x32 for a 32x32 block)
- Depth 1: Split into 4 sub-TXs (e.g., 16x16)
- Depth 2: Further split (e.g., 8x8)

At each depth, the best TX type is found, then RD cost is compared across depths. The depth with the lowest total RD cost wins.

### 7. Key Configuration Parameters

All parameters below are set in `enc_mode_config.c` based on `EncMode` (speed preset), slice type, temporal layer, resolution, QP, and screen content classification.

#### Intra Controls (`IntraCtrls`)

- `enable_intra`: Master switch for intra mode evaluation.
- `intra_mode_end`: Last intra mode to test (DC_PRED..PAETH_PRED).
- Angular prediction level (1-4): Controls which angular modes and angle deltas to test.
- `prune_using_edge_info`: Restrict to DC-only based on block variance analysis.

#### Inter Compound Controls (`InterCompCtrls`)

- `tot_comp_types`: 0 (off), 1 (AVG only), 2 (+DIST), 3 (+DIFF+WEDGE), 4 (all)
- `do_me`, `do_pme`, `do_nearest_nearest`, `do_near_near`, `do_3x3_bi`, `do_global`: Per-source enable flags
- `pred0_to_pred1_mult`: SAD-based pruning of compound candidates
- `max_mv_length`: Skip compound if MV magnitude exceeds threshold
- `use_rate`, `no_sym_dist`: Cost computation options

#### NIC Controls (`NicScalingCtrls` + `NicPruningCtrls`)

- `stage1_scaling_num`, `stage2_scaling_num`, `stage3_scaling_num`: Numerators (denominator = 16)
- `mds1_class_th`, `mds2_class_th`, `mds3_class_th`: Class pruning thresholds
- `mds1_cand_th`, `mds2_cand_th`, `mds3_cand_th`: Candidate-level pruning thresholds
- `mds1_band_cnt`, `mds2_band_cnt`: Band count for graduated pruning

#### Reference Pruning (`RefPruningControls`)

- `max_dev_to_best[TOT_INTER_GROUP]`: Maximum distortion deviation from best reference per candidate group
- `closest_refs[TOT_INTER_GROUP]`: Whether to limit to closest reference (LAST/BWD) per group
- `check_closest_multiplier`: Multiplier for closest-reference distance check

#### Sub-Pel Search (`MdSubPelSearchCtrls`)

- `subpel_search_type`: 1 (2-tap), 2 (4-tap), 3 (8-tap)
- `max_precision`: HALF_PEL, QUARTER_PEL, or EIGHTH_PEL
- `subpel_search_method`: SUBPEL_TREE or SUBPEL_TREE_PRUNED
- `pred_variance_th`: Skip sub-pel if prediction variance is below threshold
- `round_dev_th`: Early exit if improvement between rounds is below threshold
- `skip_zz_mv`, `skip_diag_refinement`, `min_blk_sz`: Various skip conditions

#### Depth Refinement (`DepthRefinementCtrls`)

- `mode`: 0 (no restriction), 1 (adaptive), 2 (pred-part only)
- `s1_parent_to_current_th`, `e1_sub_to_current_th`: Deviation thresholds for depth pruning
- `cost_band_based_modulation`: Adjust thresholds based on absolute cost
- `lower_depth_split_cost_th`: Skip parent if split rate is very low
- `split_rate_th`: Skip child if split rate exceeds percentage of current cost
- `q_weight`: QP-based threshold modulation

#### RDOQ Controls (`RdoqCtrls`)

- `enabled`: Master switch
- `skip_uv`: Skip RDOQ for chroma
- `dct_dct_only`: Only apply RDOQ for DCT_DCT transform type
- `eob_th`: Maximum EOB beyond which RDOQ is disabled
- `cut_off_num`/`cut_off_denum`: Limit RDOQ to a percentage of coefficients near EOB

## Key Functions

### Partition Search Entry Points

| Function | Location | Description |
|----------|----------|-------------|
| `svt_aom_pick_partition` | `product_coding_loop.c:10658` | Full recursive partition search with depth refinement, NSQ, and all pruning |
| `svt_aom_pick_partition_lpd0` | `product_coding_loop.c:10143` | Light PD0 partition search: SQ-only, ME-distortion-guided |
| `svt_aom_pick_partition_lpd1` | `product_coding_loop.c:10233` | Light PD1 partition search: intermediate complexity |

### Block Encoding

| Function | Location | Description |
|----------|----------|-------------|
| `md_encode_block` | `product_coding_loop.c:8820` | Full MD for one block: MVP gen, ME refinement, ref pruning, PME, candidate generation, MDS0-MDS3, final mode decision |
| `md_encode_block_light_pd0` | `product_coding_loop.c:7770` | LPD0 block encoding: minimal candidates, no sub-pel, no TXS |
| `md_encode_block_light_pd1` | `product_coding_loop.c:8496` | LPD1 block encoding: intermediate candidates, basic TXS |

### MD Stages

| Function | Location | Description |
|----------|----------|-------------|
| `md_stage_0` | `product_coding_loop.c:1606` | Full MDS0: prediction + fast cost for all candidates in a class |
| `md_stage_0_light_pd0` | `product_coding_loop.c:1471` | LPD0 fast loop |
| `md_stage_0_light_pd1` | `product_coding_loop.c:1490` | LPD1 fast loop |
| `md_stage_1` | `product_coding_loop.c:6569` | MDS1: refined evaluation with full-loop core |
| `md_stage_2` | `product_coding_loop.c:6590` | MDS2: near-final evaluation |
| `md_stage_3` | `product_coding_loop.c:6689` | MDS3: full RDO with TX search and chroma |
| `md_stage_3_light_pd0` | `product_coding_loop.c:6652` | LPD0 final stage |
| `md_stage_3_light_pd1` | `product_coding_loop.c:6671` | LPD1 final stage |

### Candidate Generation

| Function | Location | Description |
|----------|----------|-------------|
| `generate_md_stage_0_cand` | `mode_decision.c:3795` | Full candidate generation: intra + inter + compound + palette + intraBC + classification |
| `generate_md_stage_0_cand_light_pd0` | `mode_decision.c:3721` | LPD0: minimal intra + ME-based inter only |
| `generate_md_stage_0_cand_light_pd1` | `mode_decision.c:3753` | LPD1: intra + inter with duplicate filtering |
| `inject_intra_candidates` | `mode_decision.c:3418` | Inject DC, angular, smooth, PAETH modes |
| `inject_new_candidates` | `mode_decision.c:2546` | Inject NEWMV/NEW_NEWMV from ME results + WM/OBMC/II |
| `inject_mvp_candidates_ii` | `mode_decision.c:1539` | Inject NEAREST/NEAR/compound MVP modes |
| `inject_global_candidates` | `mode_decision.c:2670` | Inject GLOBALMV/GLOBAL_GLOBALMV |
| `inject_pme_candidates` | `mode_decision.c:2790` | Inject NEWMV/NEW_NEWMV from PME results |
| `inject_palette_candidates` | `mode_decision.c:3583` | Inject palette modes |
| `inject_intra_bc_candidates` | `mode_decision.c:3355` | Inject IntraBC candidates |
| `inj_comp_modes` | `mode_decision.c:1084` | Inject compound types (DIST, DIFF, WEDGE) for bipred |
| `inj_non_simple_modes` | `mode_decision.c:922` | Add inter-intra, warp, OBMC variants of a simple-trans candidate |

### Cost Functions

| Function | Location | Description |
|----------|----------|-------------|
| `svt_aom_intra_fast_cost` | `rd_cost.c:549` | MDS0 cost for intra candidates |
| `svt_aom_inter_fast_cost` | `rd_cost.c:1035` | MDS0 cost for inter candidates |
| `svt_aom_full_cost` | `rd_cost.c:1379` | Full RD cost (luma + chroma distortion + all rate) |
| `svt_aom_full_cost_light_pd0` | `rd_cost.c:1360` | Simplified full cost for LPD0 |
| `svt_av1_cost_coeffs_txb` | `rd_cost.c:365` | Coefficient rate estimation for one transform block |
| `svt_av1_mv_bit_cost` | `rd_cost.c:70` | Full MV rate cost |
| `svt_av1_mv_bit_cost_light` | `rd_cost.c:62` | Approximate MV rate cost |
| `svt_aom_partition_rate_cost` | `rd_cost.c:1854` | Partition type signaling cost |
| `svt_aom_get_intra_uv_fast_rate` | `rd_cost.c:499` | Chroma intra mode rate for fast cost |

### Transform Search

| Function | Location | Description |
|----------|----------|-------------|
| `tx_type_search` | `product_coding_loop.c:4331` | Search TX types with SATD pre-screening and early exit |
| `perform_tx_partitioning` | `product_coding_loop.c:5023` | Search TX depth (split vs. non-split) |
| `perform_dct_dct_tx` | `product_coding_loop.c:5324` | DCT_DCT-only transform path |
| `perform_dct_dct_tx_light_pd1` | `product_coding_loop.c:5175` | Simplified DCT_DCT for LPD1 |
| `perform_tx_light_pd0` | `product_coding_loop.c:4163` | Minimal transform for LPD0 |

### Motion Search (within MD)

| Function | Location | Description |
|----------|----------|-------------|
| `md_sq_motion_search` | `product_coding_loop.c:2296` | SQ-block ME refinement in MD with multi-level sparse search |
| `md_nsq_motion_search` | `product_coding_loop.c:2045` | NSQ-block ME refinement |
| `md_full_pel_search` | `product_coding_loop.c:1879` | Full-pel search kernel |
| `pme_search` | `product_coding_loop.c:3053` | Predictive ME: search around each MVP |
| `read_refine_me_mvs` | `product_coding_loop.c:2678` | Read ME MVs and optionally refine |
| `single_motion_search` | `mode_decision.c:2136` | Sub-pel refinement for a single reference |
| `svt_aom_wm_motion_refinement` | `mode_decision.c:1940` | Warped motion MV refinement |
| `svt_aom_obmc_motion_refinement` | `mode_decision.c:2250` | OBMC motion refinement |

### NIC Pruning

| Function | Location | Description |
|----------|----------|-------------|
| `set_md_stage_counts` | `product_coding_loop.c:1358` | Derive NIC counts per stage per class |
| `svt_aom_set_nics` | `product_coding_loop.c:1322` | Scale base NIC counts by scaling numerators and QP |
| `post_mds0_nic_pruning` | `product_coding_loop.c:7339` | Class + candidate pruning after MDS0 |
| `post_mds1_nic_pruning` | `product_coding_loop.c:7411` | Pruning after MDS1 |
| `post_mds2_nic_pruning` | `product_coding_loop.c:7495` | Pruning after MDS2 |

### Signal Derivation

| Function | Location | Description |
|----------|----------|-------------|
| `svt_aom_sig_deriv_enc_dec_default` | `enc_mode_config.c` | Derive all MD signals for default (VOD) mode |
| `svt_aom_sig_deriv_enc_dec_rtc` | `enc_mode_config.c` | Derive MD signals for RTC mode |
| `svt_aom_sig_deriv_enc_dec_allintra` | `enc_mode_config.c` | Derive MD signals for all-intra mode |
| `svt_aom_sig_deriv_block` | `enc_mode_config.c` | Per-block signal derivation (SB-level adaptation) |
| `svt_aom_sig_deriv_enc_dec_common` | `enc_mode_config.c` | Common signal derivation shared across modes |

### Rate Estimation Setup

| Function | Location | Description |
|----------|----------|-------------|
| `svt_aom_estimate_syntax_rate` | `md_rate_estimation.c` | Build all syntax element rate tables from frame CDFs |
| `svt_aom_estimate_coefficients_rate` | `md_rate_estimation.c` | Build coefficient-level rate tables from CDFs |
| `svt_aom_estimate_mv_rate` | `md_rate_estimation.c` | Build MV component rate tables from CDFs |

### Final Mode Decision

| Function | Location | Description |
|----------|----------|-------------|
| `svt_aom_product_full_mode_decision` | `mode_decision.c:4088` | Select best candidate, write to BlkStruct, optionally use SSIM cost |
| `svt_aom_product_full_mode_decision_light_pd1` | `mode_decision.c:3958` | LPD1 variant: simplified symbol writing |
| `product_full_mode_decision_light_pd0` | `mode_decision.c:124` | LPD0 variant: minimal symbol writing |

## Dependencies

### Upstream (inputs to MD)

| Component | Data Provided |
|-----------|---------------|
| Motion Estimation (PA) | ME MVs, ME distortion, candidate lists per block |
| TPL (Temporal Prediction Layer) | R0 delta QP, inter-intra statistics, lambda modulation |
| Picture Analysis | Variance per block, edge detection, screen content classification |
| Rate Control | QP, lambda weights, segmentation map |
| Reference Picture Management | Reference picture buffers, scaling info |
| Global Motion Estimation | Frame-level warp parameters per reference |
| Partitioning Detector (PD) | PD level selection (LPD0, LPD1, or full) per SB |

### Downstream (outputs from MD)

| Component | Data Consumed |
|-----------|---------------|
| Enc-Dec Process | Winning mode per block, quantized coefficients (when bypass_encdec), partition structure |
| Entropy Coding | `BlkStruct`/`EcBlkStruct`: mode, MVs, TX types, coefficients, reference frames, DRL context |
| Loop Filters | Block mode info for deblocking and CDEF decisions |
| Restoration | Statistics for Wiener/SGR parameter selection |

### Internal Dependencies

| Module | Relationship |
|--------|-------------|
| `enc_inter_prediction.c` | Inter prediction (regular, compound, OBMC, warped) called during MD |
| `enc_intra_prediction.c` | Intra prediction called during MD |
| `transforms.c` / `inv_transforms.c` | Forward/inverse transforms for TX search |
| `full_loop.c` | Full-loop encode path (transform + quantize + recon) used by MDS1-MDS3 |
| `neighbor_arrays.c` | Neighbor context arrays updated after each block decision |
| `adaptive_mv_pred.c` | MVP table generation (`generate_av1_mvp_table`) |
| `mcomp.c` | Motion compensation utilities for sub-pel search |
| `lambda_rate_tables.c` | Lambda/QP mapping tables |

## SIMD Functions

Mode decision itself does not have dedicated SIMD source files. However, it heavily relies on SIMD-optimized functions from other modules:

| Function | SIMD Variants | Used By |
|----------|---------------|---------|
| `svt_av1_txb_init_levels` | SSE4.1, AVX2, AVX-512, NEON | `svt_av1_cost_coeffs_txb` -- initialize coefficient level buffer |
| `svt_av1_get_nz_map_contexts` | SSE2, NEON | `svt_av1_cost_coeffs_txb` -- compute non-zero coefficient contexts |
| `svt_aom_subtract_block` | SSE2, AVX2, NEON | Residual computation in inter-intra search |
| `svt_aom_highbd_subtract_block` | SSE2, NEON | HBD residual computation |
| SAD/SATD functions | SSE2, AVX2, AVX-512, NEON | MDS0 distortion (via `svt_aom_sad_*`, `svt_aom_satd_*`) |
| SSD/SSE functions | SSE2, AVX2, NEON | Full distortion (via `svt_aom_sse`, `svt_spatial_full_distortion_*`) |
| Inter prediction | SSSE3, AVX2, AVX-512, NEON | All inter-predicted candidates |
| Intra prediction | SSE2, AVX2, NEON | All intra-predicted candidates |
| Forward transforms | SSE4.1, AVX2, AVX-512, NEON | TX search (`av1_fwd_txfm2d_*`) |
| Inverse transforms | SSE4.1, AVX2, NEON | Reconstruction in full-loop |
| Quantization | AVX2, AVX-512, NEON | `svt_aom_quantize_*` in full-loop |
| `svt_pme_sad_loop_kernel` | AVX2, AVX-512 | PME full-pel search (`md_full_pel_search`) |
| `svt_aom_combine_interintra` | SSSE3, AVX2, NEON | Inter-intra compound blending |
| `pick_wedge_fixed_sign` | SSE2, AVX2 | Wedge mask selection in compound search |
| CFL prediction | SSE4.1, AVX2, NEON | CfL alpha search and prediction |
| SSIM distortion | SSE4.1, AVX2 | `svt_spatial_full_distortion_ssim_kernel` for SSIM-tuned cost |
