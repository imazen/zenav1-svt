# Motion Estimation

## Overview

SVT-AV1 motion estimation is a hierarchical, multi-stage pipeline that finds the best motion vectors (MVs) for each block in a frame by comparing it against reference frames. The system operates at the superblock level (64x64 blocks, called "B64") and produces MVs for multiple partition sizes (8x8, 16x16, 32x32, 64x64) simultaneously.

The pipeline has these major stages:

1. **Pre-HME** -- Optional very-wide search on 1/16-resolution images to detect large motion.
2. **HME Level 0** -- Coarse search on 1/16-resolution (sixteenth) downsampled pictures.
3. **HME Level 1** -- Refined search on 1/4-resolution (quarter) downsampled pictures, centered on Level 0 results.
4. **HME Level 2** -- Refined search on full-resolution pictures, centered on Level 1 results.
5. **Reference pruning** -- Discard reference frames whose HME SAD is too far from the best.
6. **Integer-pel search** -- Full-pel exhaustive search over a rectangular search area centered on the HME search center.
7. **ME reference pruning** -- Discard references whose integer ME SAD is too far from the best.
8. **Candidate construction** -- Pack the best MVs per block partition into ME result structures consumed by mode decision.
9. **Sub-pel refinement** -- (Performed later in Mode Decision) Refine integer MVs to half-pel, quarter-pel, and eighth-pel precision.
10. **Global motion estimation** -- Frame-level motion model fitting (translation, rotation+zoom, affine) via corner detection, feature matching, RANSAC, and iterative refinement.

Additionally, the codebase includes:
- **Hash-based motion search** for IntraBC (intra block copy).
- **Warped motion** parameter estimation for local affine models at block level.
- **Adaptive MV prediction** -- generation of MV candidate lists from spatial/temporal neighbors.

## Source Files

| Path | Purpose |
|------|---------|
| `Source/Lib/Codec/motion_estimation.c` | Main ME engine: SAD kernels, HME levels 0/1/2, integer full-pel search, pre-HME, candidate construction, top-level `svt_aom_motion_estimation_b64()` |
| `Source/Lib/Codec/motion_estimation.h` | Declarations for ME entry points, SAD functions, interpolation filter coefficients, partition tables |
| `Source/Lib/Codec/av1me.c` | AV1-specific ME: search site initialization, full-pixel diamond search for IntraBC, MV cost functions, hash-based IntraBC search |
| `Source/Lib/Codec/av1me.h` | SearchSite/SearchSiteConfig structs, variance function pointer table, full-pixel search declarations |
| `Source/Lib/Codec/me_context.c` | MeContext constructor and destructor |
| `Source/Lib/Codec/me_context.h` | MeContext struct (the central ME state), MePredictionUnit, search area controls, HME/pre-HME controls |
| `Source/Lib/Codec/me_process.c` | ME pipeline kernel: thread entry point `svt_aom_motion_estimation_kernel()`, dispatches ME/TF/GM tasks |
| `Source/Lib/Codec/me_process.h` | MotionEstimationContext_t struct, kernel declaration |
| `Source/Lib/Codec/mcomp.c` | Sub-pixel motion search: `svt_av1_find_best_sub_pixel_tree`, `svt_av1_find_best_sub_pixel_tree_pruned`, MV cost computation |
| `Source/Lib/Codec/mcomp.h` | MV cost types, subpel search parameter structs (SUBPEL_MOTION_SEARCH_PARAMS, svt_mv_cost_param, MSBuffers) |
| `Source/Lib/Codec/global_me.c` | Global motion estimation orchestrator: `svt_aom_global_motion_estimation()`, pre-processor pass, per-reference GM computation |
| `Source/Lib/Codec/global_me.h` | GM function declarations |
| `Source/Lib/Codec/global_me_cost.c` | `svt_aom_gm_get_params_cost()` -- bitstream cost of GM parameters |
| `Source/Lib/Codec/global_me_cost.h` | GM cost declaration |
| `Source/Lib/Codec/global_motion.c` | GM model fitting: `svt_av1_refine_integerized_param()`, `svt_av1_convert_model_to_params()`, correspondence generation from corners/MVs, RANSAC dispatch |
| `Source/Lib/Codec/global_motion.h` | Correspondence struct, MotionModel struct, GM function declarations |
| `Source/Lib/Codec/hash_motion.c` | Hash-based block matching for IntraBC: hash table management, CRC32-based block hashing, hierarchical hash map construction |
| `Source/Lib/Codec/hash_motion.h` | BlockHash, HashTable structs, hash generation/lookup declarations |
| `Source/Lib/Codec/warped_motion.c` | Warped motion core: warp plane, find_projection (local warp parameter estimation), shear parameter derivation, warped interpolation filter table |
| `Source/Lib/Codec/warped_motion.h` | Warp model precision constants, warped filter coefficients, warp function declarations |
| `Source/Lib/Codec/enc_warped_motion.c` | `svt_av1_warp_error()` -- compute SAD between warped reference and source for GM refinement |
| `Source/Lib/Codec/enc_warped_motion.h` | Warp error function declaration |
| `Source/Lib/Codec/corner_match.c` | Feature correspondence: NCC-based corner matching between frame and reference, iterative refinement of matches |
| `Source/Lib/Codec/corner_match.h` | `svt_av1_determine_correspondence()` declaration |
| `Source/Lib/Codec/corner_detect.c` | FAST-9 corner detection wrapper: `svt_av1_fast_corner_detect()` |
| `Source/Lib/Codec/corner_detect.h` | Corner detection declaration |
| `Source/Lib/Codec/adaptive_mv_pred.c` | MV prediction candidate generation: spatial/temporal MV scanning, MV stack construction, global MV derivation, warped motion parameter estimation for blocks |
| `Source/Lib/Codec/adaptive_mv_pred.h` | MV prediction declarations: `svt_aom_generate_av1_mvp_table()`, `svt_aom_get_av1_mv_pred_drl()`, warped motion parameter estimation |
| `Source/Lib/Codec/ransac.c` | RANSAC implementation for global motion: translation, rotation+zoom, affine model fitting with iterative refinement |
| `Source/Lib/Codec/ransac.h` | RANSAC structs (RANSAC_MOTION, RansacModelInfo), `svt_aom_ransac()` declaration |
| `Source/Lib/Codec/me_results.c` | MotionEstimationResults object constructor (pipeline message) |
| `Source/Lib/Codec/me_results.h` | MotionEstimationResults struct |
| `Source/Lib/Codec/me_sad_calculation.c` | `svt_initialize_buffer_32bits_c()` -- buffer initialization utility |
| `Source/Lib/Codec/me_sad_calculation.h` | Buffer initialization declaration |

## Test Coverage

| Test File | What It Tests |
|-----------|---------------|
| `test/MotionEstimationTest.cc` | SAD kernel correctness: verifies AVX2 implementations of `svt_aom_sadWxH` and `svt_aom_sadWxHx4d` match C reference for all 22 standard block sizes (4x4 through 128x128). Tests both single-reference and 4-reference SAD variants. |
| `test/SadTest.cc` | Comprehensive SAD/ME primitive testing: `svt_nxm_sad_kernel`, `svt_sad_loop_kernel`, `svt_ext_all_sad_calculation_8x8_16x16`, `svt_ext_eight_sad_calculation_32x32_64x64`, `svt_ext_sad_calculation_8x8_16x16`, `svt_ext_sad_calculation_32x32_64x64`, `svt_initialize_buffer_32bits`, `svt_aom_sad_16bit_kernel`, `svt_pme_sad_loop_kernel`. Tests with multiple block sizes and patterns (REF_MAX, SRC_MAX, RANDOM, UNALIGN). |
| `test/SatdTest.cc` | SATD (Sum of Absolute Transformed Differences) correctness: tests `svt_aom_satd` for constant, random, and extreme inputs at various transform sizes. Verifies SIMD matches C reference. |
| `test/corner_match_test.cc` | Cross-correlation computation correctness: verifies `svt_av1_compute_cross_correlation` SIMD implementations match C reference. Tests with random image data for match sizes 5 and 7. Includes performance benchmarks. |
| `test/GlobalMotionUtilTest.cc` | RANSAC model fitting correctness: tests translation, rotation+zoom, and affine model recovery. Generates synthetic correspondences with known affine transforms, adds outlier noise, and verifies that RANSAC recovers parameters within tolerance. Tests both with and without added noise/outliers. |

## Data Structures

### MeContext (me_context.h)

The central state object for all ME operations on a single 64x64 superblock.

| Field | Type | Meaning |
|-------|------|---------|
| `interpolated_full_stride[list][ref]` | `uint32_t` | Stride of the integer search buffer for each reference |
| `me_distortion[SQUARE_PU_COUNT]` | `uint32_t` | Best ME distortion per square partition |
| `b64_src_ptr` | `uint8_t*` | Pointer to source block (64x64) |
| `b64_src_stride` | `uint32_t` | Source stride |
| `quarter_b64_buffer` | `uint8_t*` | 1/4-resolution source block |
| `sixteenth_b64_buffer` | `uint8_t*` | 1/16-resolution source block |
| `integer_buffer_ptr[list][ref]` | `uint8_t*` | Pointer into reference at search region origin |
| `p_best_sad_8x8..64x64` | `uint32_t*` | Pointers into per-ref best SAD arrays (set before each search) |
| `p_best_mv8x8..64x64` | `uint32_t*` | Pointers into per-ref best MV arrays |
| `p_sb_best_sad[list][ref][SQUARE_PU_COUNT]` | `uint32_t` | Per-reference, per-partition best SAD values |
| `p_sb_best_mv[list][ref][SQUARE_PU_COUNT]` | `uint32_t` | Per-reference, per-partition best MVs (packed: y<<16 | x) |
| `p_eight_sad16x16[16][8]` | `uint32_t` | Intermediate SAD values for 8-at-a-time search (16x16 blocks) |
| `p_eight_sad32x32[4][8]` | `uint32_t` | Intermediate SAD values for 8-at-a-time search (32x32 blocks) |
| `hme_search_method` | `uint8_t` | FULL_SAD_SEARCH or SUB_SAD_SEARCH |
| `me_search_method` | `uint8_t` | FULL_SAD_SEARCH or SUB_SAD_SEARCH |
| `enable_hme_flag` | `bool` | Master HME enable |
| `enable_hme_level0_flag` | `bool` | Enable HME level 0 (1/16 resolution) |
| `enable_hme_level1_flag` | `bool` | Enable HME level 1 (1/4 resolution) |
| `enable_hme_level2_flag` | `bool` | Enable HME level 2 (full resolution) |
| `me_sa` | `SearchAreaMinMax` | Min/max ME search area dimensions |
| `hme_l0_sa` | `SearchAreaMinMax` | HME Level 0 search area dimensions |
| `hme_l1_sa` | `SearchArea` | HME Level 1 search area per L0 center |
| `hme_l2_sa` | `SearchArea` | HME Level 2 search area per L1 center |
| `search_results[list][ref]` | `SearchResults` | HME search center and SAD per reference |
| `prehme_data[list][ref][SEARCH_REGION_COUNT]` | `SearchInfo` | Pre-HME results per reference per search region |
| `prehme_ctrl` | `PreHmeCtrls` | Pre-HME control parameters |
| `me_hme_prune_ctrls` | `MeHmeRefPruneCtrls` | Reference pruning thresholds |
| `me_sr_adjustment_ctrls` | `MeSrCtrls` | Search region size adjustment controls |
| `me_8x8_var_ctrls` | `Me8x8VarCtrls` | 8x8 SAD variance-based search area adjustment |
| `me_type` | `EbMeType` | ME_CLOSE_LOOP, ME_MCTF, ME_TPL, ME_OPEN_LOOP, ME_FIRST_PASS, ME_DG_DETECTOR |
| `num_of_list_to_search` | `uint8_t` | Number of reference lists (1 or 2) |
| `num_of_ref_pic_to_search[2]` | `uint8_t` | Number of references per list |
| `zz_sad[list][ref]` | `uint32_t` | Zero-motion (0,0) SAD per reference |
| `b64_width`, `b64_height` | `uint32_t` | Actual block dimensions (may be < 64 at frame edges) |
| `tf_*` fields | various | Temporal filtering-specific motion data |

### MePredictionUnit (me_context.h)

```c
typedef struct MePredictionUnit {
    uint64_t distortion;      // Best distortion for this PU
    int16_t  x_mv;            // Horizontal MV component
    int16_t  y_mv;            // Vertical MV component
    uint32_t sub_pel_direction; // Sub-pel refinement direction
} MePredictionUnit;
```

### SearchSite / SearchSiteConfig (av1me.h)

Used for diamond/hex search patterns in IntraBC full-pixel search.

```c
typedef struct SearchSite {
    Mv  mv;      // MV offset for this search site
    int offset;  // Linear buffer offset = mv.y * stride + mv.x
} SearchSite;

typedef struct SearchSiteConfig {
    SearchSite ss[8 * MAX_MVSEARCH_STEPS + 1]; // All search sites across all steps
    int        ss_count;                         // Total number of search sites
    int        searches_per_step;                // Sites per step (8 for 3-step search)
} SearchSiteConfig;
```

### svt_mv_cost_param (mcomp.h)

Parameters for computing the rate cost of a motion vector during sub-pel search.

```c
typedef struct svt_mv_cost_param {
    const Mv*    ref_mv;        // Reference MV (predictor) for cost computation
    Mv           full_ref_mv;   // Full-pel reference MV
    MV_COST_TYPE mv_cost_type;  // MV_COST_ENTROPY, MV_COST_L1_LOWRES/MIDRES/HDRES, MV_COST_OPT, MV_COST_NONE
    const int*   mvjcost;       // Joint MV component cost table
    const int*   mvcost[2];     // Per-component MV cost tables [row, col]
    int          error_per_bit; // Multiplier converting rate to distortion cost
    int          early_exit_th; // Early exit threshold for MV_COST_OPT mode
    int          sad_per_bit;   // Multiplier converting rate to SAD cost
} svt_mv_cost_param;
```

### SUBPEL_MOTION_SEARCH_PARAMS (mcomp.h)

All parameters needed for a single sub-pixel motion search invocation.

```c
typedef struct {
    int               allow_hp;           // Allow high-precision (1/8-pel) MVs
    SUBPEL_FORCE_STOP forced_stop;        // Earliest sub-pel precision to stop at
    int               iters_per_step;     // Iterations per refinement step
    int               pred_variance_th;   // Skip subpel if prediction variance below this
    uint8_t           abs_th_mult;        // Absolute threshold multiplier for early exit
    int               round_dev_th;       // Round deviation threshold for early termination
    uint8_t           skip_diag_refinement; // Level of diagonal refinement skipping
    SUBPEL_STAGE      search_stage;       // SPEL_ME or SPEL_PME
    uint8_t           list_idx;           // Reference list index
    uint8_t           ref_idx;            // Reference index within list
    SubpelMvLimits    mv_limits;          // Sub-pel MV range limits
    svt_mv_cost_param mv_cost_params;     // MV cost parameters
    SUBPEL_SEARCH_VAR_PARAMS var_params;  // Variance/distortion computation parameters
} SUBPEL_MOTION_SEARCH_PARAMS;
```

### Correspondence (global_motion.h)

A matched feature point pair between source and reference frames.

```c
typedef struct {
    int x, y;   // Source frame coordinates
    int rx, ry; // Reference frame coordinates
} Correspondence;
```

### MotionModel (global_motion.h)

```c
typedef struct {
    double params[MAX_PARAMDIM]; // Model parameters [tx, ty, a, b, c, d]
    int*   inliers;              // Array of inlier point coordinates
    int    num_inliers;          // Number of inliers
} MotionModel;
```

### RANSAC_MOTION (ransac.h)

Internal RANSAC state for a single candidate motion model.

```c
typedef struct {
    int    num_inliers;     // Number of inlier correspondences
    double sse;             // Sum of squared errors for inliers
    int*   inlier_indices;  // Indices into the correspondence array
} RANSAC_MOTION;
```

### BlockHash (hash_motion.h)

```c
typedef struct _block_hash {
    int16_t  x;            // Block x position from top-left of picture
    int16_t  y;            // Block y position from top-left of picture
    uint32_t hash_value2;  // Secondary hash for verification
} BlockHash;
```

### HashTable (hash_motion.h)

```c
typedef struct HashTable {
    Vector** p_lookup_table; // Array of 2^(crc_bits + block_size_bits) buckets
} HashTable;
```

### SearchArea / SearchAreaMinMax / SearchInfo / SearchResults (me_context.h)

```c
typedef struct SearchArea {
    uint16_t width;  // Search area width in pixels
    uint16_t height; // Search area height in pixels
} SearchArea;

typedef struct SearchAreaMinMax {
    SearchArea sa_min; // Minimum search area (base)
    SearchArea sa_max; // Maximum search area (scaled by distance)
} SearchAreaMinMax;

typedef struct SearchInfo {
    SearchArea sa;       // Search area dimensions used
    Mv         best_mv;  // Best MV found
    uint64_t   sad;      // Best SAD found
    uint8_t    valid;    // 1 if results are valid
} SearchInfo;

typedef struct SearchResults {
    uint8_t  list_i;    // Reference list index
    uint8_t  ref_i;     // Reference index within list
    int16_t  hme_sc_x;  // HME search center x
    int16_t  hme_sc_y;  // HME search center y
    uint64_t hme_sad;   // HME SAD
    uint8_t  do_ref;    // Whether to process this reference in ME
} SearchResults;
```

### MotionEstimationContext_t (me_process.h)

Thread-level context wrapping the ME pipeline.

```c
typedef struct MotionEstimationContext {
    EbFifo*    picture_decision_results_input_fifo_ptr;  // Input FIFO
    EbFifo*    motion_estimation_results_output_fifo_ptr; // Output FIFO
    MeContext* me_ctx;        // Per-block ME state
    uint8_t*   index_table0;  // Index tables (unused in current code)
    uint8_t*   index_table1;
} MotionEstimationContext_t;
```

### AomVarianceFnPtr (av1me.h)

Virtual function table for block-size-specific distortion functions.

```c
typedef struct aom_variance_vtable {
    AomSadFn                sdf;       // SAD function
    AomVarianceFn           vf;        // Variance function (8-bit)
    AomVarianceFn           vf_hbd_10; // Variance function (10-bit HBD)
    AomSubpixVarianceFn     svf;       // Sub-pixel variance (bilinear filter)
    AomSadMultiDFn          sdx4df;    // SAD for 4 references simultaneously
    AomObmcSadFn            osdf;      // OBMC SAD
    AomObmcVarianceFn       ovf;       // OBMC variance
    AomObmcSubpixvarianceFn osvf;      // OBMC sub-pixel variance
} AomVarianceFnPtr;
```

## Algorithms

### 1. Hierarchical Motion Estimation (HME)

The HME pipeline searches for block motion at progressively higher resolutions, using the result from each level as the search center for the next.

#### Pre-HME (`prehme_core`)

1. Operate on 1/16-resolution source and reference pictures.
2. For each search region (up to `SEARCH_REGION_COUNT = 2`):
   a. Compute search area origin centered at (0,0).
   b. Clip search area to reference picture boundaries (accounting for padding).
   c. Call `svt_sad_loop_kernel()` -- an exhaustive SAD search across the entire search area.
   d. Scale the resulting MV by 4 (to convert from 1/4 resolution to full resolution units, since sixteenth = 1/4 of 1/4).
3. Early exit conditions:
   - If zero-motion SAD (`zz_sad`) is below `me_early_exit_th`, skip pre-HME and use (0,0).
   - If L1 early exit is enabled and L0 reference had small motion/SAD, mirror L0's MV.

#### HME Level 0 (`hme_level_0`)

1. Operate on 1/16-resolution pictures.
2. Input: block position at sixteenth resolution, search area dimensions.
3. Search area centered at origin offset by `(sr_w, sr_h)` for multi-region support.
4. Clip search area to reference boundaries.
5. Round search area width to multiple of 8 (for SIMD efficiency).
6. Call `svt_sad_loop_kernel()` with either full-height or half-height (SUB_SAD mode skips every other row and doubles the result).
7. Output MV is scaled by 4 (sixteenth to quarter resolution).

#### HME Level 1 (`hme_level_1`)

1. Operate on 1/4-resolution pictures.
2. Search area centered on the HME Level 0 search center.
3. Same boundary clipping and rounding as Level 0.
4. Call `svt_sad_loop_kernel()`.
5. Output MV is scaled by 2 (quarter to full resolution).

#### HME Level 2 (`hme_level_2`)

1. Operate on full-resolution pictures.
2. Search area centered on the HME Level 1 search center.
3. Same boundary clipping pattern.
4. Call `svt_sad_loop_kernel()`.
5. Output MV is at full-pel resolution.

#### Zero-MV Check (`check_00_center`)

After HME, compare the HME search center's SAD against the zero-motion (0,0) SAD. If (0,0) is cheaper (using a simple cost model: `SAD << COST_PRECISION`), replace the HME center with (0,0).

### 2. Integer Full-Pel Search (`integer_search_b64`)

For each reference frame that passed HME pruning:

1. **Determine search area dimensions:**
   - Start with `me_sa.sa_min` base dimensions.
   - Scale by temporal distance: `width = min(width * scaled_dist, sa_max.width)`.
   - Apply MV-based search area adjustment if the HME center MV is large.
   - Apply screen-content boost if enabled (multiply by factors from `search_area_multipliers` table based on HME SAD level).
   - Reduce by `reduce_me_sr_divisor` (from HME-based SR reduction).
   - Round width to multiple of 8, enforce minimum height of 3.

2. **8x8 variance-based adjustment** (if `me_8x8_var_ctrls.enabled`):
   - Search only the center point first.
   - Compute variance of 8x8 SADs across the 64x64 block.
   - If variance is very low (homogeneous motion), shrink the search area.
   - If variance is very high (complex motion), grow the search area.

3. **Clip search area** to picture boundaries (accounting for ME_FILTER_TAP/2 padding).

4. **Perform exhaustive search** via `open_loop_me_fullpel_search_sblock()`:
   - Iterate over each row of the search area.
   - Process 8 horizontal positions at a time (`open_loop_me_get_eight_search_point_results_block`).
   - At each position, compute SAD for all 8x8 sub-blocks, combine into 16x16, 32x32, and 64x64.
   - Track best SAD and MV for each partition size independently.

### 3. SAD Computation (Bottom-Up Aggregation)

The SAD computation uses a hierarchical bottom-up approach:

1. **8x8 SAD** (`svt_aom_compute8x4_sad_kernel_c` or `compute8x8_sad_kernel_c`):
   - Compute pixel-by-pixel absolute differences and sum.
   - In SUB_SAD mode: compute 8x4 on every-other-row and multiply by 2.

2. **16x16 SAD** (`svt_ext_sad_calculation_8x8_16x16`):
   - Sum of 4 adjacent 8x8 SADs.
   - Track best per 8x8 and per 16x16.

3. **32x32 SAD** (`svt_ext_sad_calculation_32x32_64x64`):
   - Sum of 4 adjacent 16x16 SADs.

4. **64x64 SAD**:
   - Sum of 4 adjacent 32x32 SADs.

5. **Eight-at-a-time variant** (`svt_ext_all_sad_calculation_8x8_16x16_c`):
   - Processes 8 consecutive horizontal search positions simultaneously.
   - Computes all 64 8x8 SADs, 16 16x16 SADs, 4 32x32 SADs, and 1 64x64 SAD per batch.
   - Stores intermediate 16x16 SADs in `p_eight_sad16x16[16][8]` for reuse by 32x32/64x64.

### 4. MV Cost Computation

MV cost is computed differently depending on the search context:

#### Entropy-based cost (`MV_COST_ENTROPY`)

```
cost = round( mv_cost(diff, mvjcost, mvcost) * error_per_bit / (2^shift) )
```

Where `mv_cost()` looks up the joint MV component and per-component costs from probability tables.

#### L1-norm cost (`MV_COST_L1_LOWRES/MIDRES/HDRES`)

```
cost = (lambda * (|diff_x| + |diff_y|)) >> 3
```

Where lambda = {2, 0, 1} for low/mid/HD resolution respectively.

#### Optimized cost (`MV_COST_OPT`)

```
cost = round( (|diff_x| + |diff_y|) << 8 * error_per_bit / (2^shift) )
```

#### Light MV cost (`svt_aom_mv_err_cost_light`)

A fast approximation used in some search contexts:

```
mv_rate = 1296 + 50 * (|diff_x| + |diff_y|)
```

### 5. Sub-Pixel Refinement

Two main sub-pixel search functions, both using iterative half-step refinement:

#### `svt_av1_find_best_sub_pixel_tree` (Full Quality)

1. **Initialization**: Compute center error at full-pel position using variance function.
2. **Early exits**:
   - If `qp * besterr < threshold` (already very good at full-pel).
   - If prediction variance is below `pred_variance_th` (flat reference, interpolation won't help).
   - If MVP-based pruning applies (deviation between ME MV and best MVP is large enough to limit sub-pel rounds).
3. **For each refinement round** (half-pel, quarter-pel, eighth-pel):
   a. **First level check** (`svt_first_level_check`):
      - Test 4 cardinal directions (left, right, up, down) at current step size.
      - Test the best diagonal direction.
      - Use upsampled prediction error (accurate interpolation).
   b. **Second level check** (`svt_second_level_check_v2`):
      - If the diagonal improved, test 2 additional positions biased toward the best direction.
      - If a further improvement is found, test one more diagonal.
   c. Halve step size: `hstep >>= 1`.

#### `svt_av1_find_best_sub_pixel_tree_pruned` (Speed-Optimized)

1. Same initialization and early exits as the full version.
2. **For each refinement round**:
   a. **Two-level check fast** (`two_level_checks_fast`):
      - First level: Test 4 cardinal directions using bilinear-filter-estimated error (faster than upsampled).
      - Test best diagonal.
      - If improved over original error, do a second level check with chess-pattern search.
   b. Additional early exit: if `skip_diag_refinement >= 4`, set `org_error = 0` to disable diagonal check. Various levels control the aggressiveness.
   c. Round deviation early exit: if `(besterr - prev_besterr) / prev_besterr >= round_dev_th`, stop refining.

#### Step Size Progression

```
Step:  4  ->  2  ->  1
Equiv: 1/2-pel -> 1/4-pel -> 1/8-pel
```

Number of rounds is: `min(FULL_PEL - forced_stop, 3 - !allow_hp)`.

### 6. Global Motion Estimation

Global motion estimates a single motion model that applies to the entire frame for each reference.

#### Pipeline (`svt_aom_global_motion_estimation`)

1. **Compute average ME SAD** across all superblocks. Use this to set `global_motion_estimation_level` (0-3), controlling how many references to process.
2. **Determine downsample level**: GM_FULL (full resolution), GM_DOWN (1/4), GM_DOWN16 (1/16), or adaptive based on ME SAD.
3. **Bypass check**: If fewer than half the superblocks allow GM (based on MV histogram analysis), skip GM.
4. **Detect corners** on the source frame using FAST-9 (if correspondence method is CORNERS).
5. **For each reference frame**:
   a. Generate correspondences (corner matching or MV-based).
   b. For each transform model (TRANSLATION through AFFINE, controlled by `search_start_model`/`search_end_model`):
      - Run RANSAC to fit the model.
      - Convert double-precision params to integer `WarpedMotionParams`.
      - Upscale translation if using downsampled detection.
      - Iteratively refine the integerized parameters (`svt_av1_refine_integerized_param`).
      - Check if the warp error is enough of an improvement over the reference SAD to justify the coding cost.
   c. If the best model for list 0 ref 0 is IDENTITY and `identity_exit` is set, skip list 1 entirely.

#### Corner Detection (`svt_av1_fast_corner_detect`)

Wrapper around the FAST-9 corner detector with non-maximum suppression:
1. Run `svt_aom_fast9_detect_nonmax()` with barrier threshold = 18.
2. Return up to `MAX_CORNERS` (4096) corner points as (x,y) pairs.

#### Feature Matching (`svt_av1_determine_correspondence`)

1. For each source corner:
   - Find the best matching reference corner within distance threshold `(max(width,height)/16)^2`.
   - Compute normalized cross-correlation (NCC) over a `match_sz x match_sz` window.
   - Accept match if `NCC^2 > template_variance * THRESHOLD_NCC^2` (threshold = 0.75).
2. Refine correspondences (`improve_correspondence`):
   - For each correspondence, search a 9x9 neighborhood around the reference point for a better NCC match.
   - Then do the same around the source point.

#### Cross-Correlation (`svt_av1_compute_cross_correlation_c`)

Computes `(cov^2 / var2)` where:
- `cov = cross * match_sz_sq - sum1 * sum2`
- `var2 = sumsq2 * match_sz_sq - sum2 * sum2`

This is equivalent to `NCC^2 * var1` (unnormalized on one side).

#### MV-Based Correspondence (`correspondence_from_mvs`)

Alternative to corner matching: use ME results as correspondences.
- For each superblock, extract the MV at the requested granularity (64x64, 32x32, 16x16, or 8x8).
- Map block center + MV to correspondence point pairs.
- Scale coordinates if using downsampled frames.

#### RANSAC (`svt_aom_ransac`)

1. **Input**: Correspondences, transformation type, desired number of motions.
2. **Configuration by model type**:
   - TRANSLATION: `find_translation` (1 point minimum), `score_translation`
   - ROTZOOM: `find_rotzoom` (2 points minimum), `score_affine`
   - AFFINE: `find_affine` (3 points minimum), `score_affine`
3. **Trial phase** (20 trials):
   - Randomly sample `minpts` correspondences using LCG random generator.
   - Fit model using least squares.
   - Score model: count inliers where projection error < 1.25 pixels (threshold squared = 1.5625).
   - Keep the best `num_desired_motions` models (sorted by inlier count, then SSE).
4. **Refinement phase** (up to 5 iterations per model):
   - Refit model using all current inliers.
   - Re-score to get new inlier set.
   - Continue while inlier count increases; stop when it plateaus.
5. **Output**: Parameters and inlier points for each model.

#### Model Fitting Functions

**Translation** (`find_translation`):
- Average displacement: `params = (mean(dx-sx), mean(dy-sy), 1, 0, 0, 1)`.

**Rotation+Zoom** (`find_rotzoom`):
- Solve 4x4 least-squares system for `[tx, ty, a, b]`.
- Model: `x' = a*x + b*y + tx`, `y' = b*x - a*y + ty` (note: `params[4] = -params[3]`, `params[5] = params[2]`).

**Affine** (`find_affine`):
- Solve two independent 3x3 least-squares systems.
- Model: `x' = a*x + b*y + tx`, `y' = c*x + d*y + ty`.

#### Parameter Refinement (`svt_av1_refine_integerized_param`)

1. Convert model to integer `wmmat[]` representation.
2. Compute initial warp error (SAD between warped reference and source).
3. For each refinement step (initial step = 16, halved each iteration):
   - For each parameter:
     - Try `param - step` and `param + step`.
     - Keep whichever gives lower warp error.
4. Recalculate the model type from the final parameters.
5. Early exit if error advantage is not sufficient (checked via `svt_av1_is_enough_erroradvantage`).

#### Warp Error (`svt_av1_warp_error`)

1. Validate shear parameters.
2. Process frame in 32x32 blocks.
3. For each block:
   - Warp the reference block using `svt_warp_plane()`.
   - Compute SAD between warped block and destination.
   - Accumulate; early exit if running total exceeds `best_error`.
4. Chess refinement mode (`chess_refn`): process every other block in a checkerboard pattern, multiply total by 2.

#### GM Error Advantage Test

```c
is_enough = (best_erroradvantage < erroradv_tr[type]) &&
            (best_erroradvantage * params_cost < erroradv_prod_tr[type]);
```

Thresholds: `erroradv_tr = {0.65, 0.50, 0.45}`, `erroradv_prod_tr = {20000, 15000, 14000}`.

### 7. Hash-Based Motion Search (IntraBC)

Used for intra block copy to find matching blocks within the same frame.

#### Hash Generation

1. **2x2 base hash** (`svt_av1_generate_block_2x2_hash_value`):
   - For each 2x2 block, pack 4 pixel values into a 32-bit hash.
   - 8-bit: identity packing `(a<<24 | b<<16 | c<<8 | d)`.
   - HBD: XOR packing of upper and lower bytes.

2. **Hierarchical CRC hashing** (`svt_av1_generate_block_hash_value`):
   - Combine 4 adjacent sub-block hashes using CRC32C to produce the next level's hash.
   - Repeat until reaching the target block size (4x4, 8x8, 16x16, etc.).

3. **Hash table construction** (`svt_aom_rtime_alloc_svt_av1_add_to_hash_map_by_row_with_precal_data`):
   - Hash value = `(crc_hash & ((1<<16)-1)) + (block_size_index << 16)`.
   - Hierarchical spatial exploration: start with stride = block_size, progressively halve.
   - State machine cycles through 4 offset combinations to ensure spatial diversity.
   - Cap per-bucket entries at `max_cand_per_bucket` (default 256).

#### Hash Lookup for IntraBC

`svt_av1_get_block_hash_value`:
1. Compute 2x2 base hashes for the current block.
2. Hierarchically combine up to the target block size using CRC32C.
3. Return `hash_value1` (bucket key) and `hash_value2` (full hash for collision verification).

### 8. Warped Motion Model

#### Local Warp Parameter Estimation (`svt_find_projection`)

1. Collect sample points from neighboring blocks' MVs using `svt_aom_select_samples()`.
2. Filter samples that are too far from the current block's MV.
3. Use least-squares fitting to derive 2x3 affine parameters from the samples.
4. Convert to `WarpedMotionParams` (16-bit fixed-point precision).
5. Validate shear parameters with `svt_get_shear_params()`.

#### Warp Plane (`svt_av1_warp_plane` / `svt_warp_plane`)

1. Apply the affine transform to each output pixel's coordinates to find source position.
2. Use 8-tap warped interpolation filter (`svt_aom_warped_filter`) at 1/64-pel precision.
3. Handle boundary conditions with padding tables (`warp_pad_left`, `warp_pad_right`).
4. Support chroma with subsampling.

#### Warp Model Precision

- `WARPEDMODEL_PREC_BITS = 16` -- precision for affine matrix elements.
- `WARPEDPIXEL_PREC_BITS = 6` -- 64 sub-pixel positions for interpolation.
- Translation clamped to `[-128*2^16, 128*2^16]`.
- Non-diagonal affine elements clamped to `[-2^13, 2^13]`.

### 9. MV Prediction Candidate Generation (`adaptive_mv_pred.c`)

#### `svt_aom_generate_av1_mvp_table`

1. **Scan spatial neighbors** in a defined order (left, above, above-right, above-left, below-left).
2. For each neighbor's MbModeInfo:
   - If it uses the same reference frame, add its MV to the reference MV stack with a weight.
   - Handle both single and compound reference frames.
   - Account for global motion: if the neighbor uses a global MV mode, substitute the global MV.
3. **Scan temporal neighbors** from the co-located reference frame.
4. **Sort the MV stack** by weight (descending).
5. **Derive nearest/near MVs** from the sorted stack.
6. **Clamp all MVs** to picture boundaries.

#### `svt_aom_get_av1_mv_pred_drl`

Extract the MV for a specific DRL (Dynamic Reference List) index from the pre-built MV stack:
- DRL index 0-1 map to NEARESTMV/NEARMV modes.
- Higher DRL indices reach deeper into the stack.

#### `svt_aom_warped_motion_parameters`

1. Initialize warped motion samples from neighboring blocks via `svt_aom_init_wm_samples`.
2. Select valid samples based on MV consistency with the current block's MV.
3. Call `svt_find_projection` to fit warp parameters.
4. Apply optional approximation shutoff for speed.

### 10. Reference Pruning

#### HME-Based Pruning (`hme_prune_ref_and_adjust_sr`)

1. Find the reference with the best HME SAD.
2. For each non-primary reference: if `(hme_sad - best) * 100 > threshold * best`, mark `do_ref = 0`.
3. Also supports pruning based on:
   - Zero-motion SAD similarity (`zz_sad_th`, `zz_sad_pct`).
   - Pre-HME SAD similarity (`phme_sad_th`, `phme_sad_pct`).

#### ME-Based Pruning (`me_prune_ref`)

After integer search:
1. Accumulate 8x8 SADs across the 64x64 block for each reference.
2. Find the reference with the best total SAD.
3. Prune references whose SAD deviates more than `prune_ref_if_me_sad_dev_bigger_than_th` percent from the best.

#### Search Region Adjustment

Based on HME results, the search region can be reduced:
- If HME MV is small and HME SAD is low (`stationary_hme_sad_abs_th`), divide search area by `stationary_me_sr_divisor`.
- If HME SAD is very low (`reduce_me_sr_based_on_hme_sad_abs_th`), divide by `me_sr_divisor_for_low_hme_sad`.
- Distance-based HME resizing scales down the HME search area for higher reference indices.

### 11. svt_sad_loop_kernel

The core search kernel used by all HME levels and pre-HME. This is a highly optimized function (with SIMD variants) that:

1. Takes source block, reference buffer, search area dimensions.
2. For each position in the search area, computes SAD between source and reference.
3. Tracks the minimum SAD and corresponding position.
4. Returns the best SAD and (x,y) offset within the search area.

### 12. ME Pipeline Kernel (`svt_aom_motion_estimation_kernel`)

The thread entry point that orchestrates all ME work:

1. Receive task from picture decision: TASK_PAME (PA ME), TASK_TFME (temporal filtering ME), TASK_DG_DETECTOR_HME, or TASK_SUPERRES_RE_ME.
2. Derive ME signals from encoder configuration.
3. For PAME/superres tasks:
   - Set up source and reference buffers (full, 1/4, 1/16 resolution).
   - For each 64x64 block in the assigned segment:
     - Prefetch source data.
     - Call `svt_aom_motion_estimation_b64()` -- the main ME function.
   - After all blocks complete: run global motion estimation if enabled.
4. For TFME tasks: invoke temporal filtering.

### 13. Top-Level ME Function (`svt_aom_motion_estimation_b64`)

1. Compute actual block dimensions (handle frame edges).
2. Initialize all ME/HME buffers and search results.
3. Run HME: `hme_b64()` -- calls pre-HME, HME L0, L1, L2 as configured.
4. For MCTF: early exit if HME SAD is very low.
5. Prune references based on HME results.
6. Run integer full-pel search: `integer_search_b64()`.
7. Prune references based on integer ME results.
8. Construct ME candidate arrays (unipred and bipred combinations).
9. Compute per-block-size distortion statistics.
10. Perform GM detection (MV histogram analysis per superblock).

## Key Functions

### motion_estimation.c

```c
EbErrorType svt_aom_motion_estimation_b64(
    PictureParentControlSet* pcs, uint32_t b64_index,
    uint32_t b64_origin_x, uint32_t b64_origin_y,
    MeContext* me_ctx, EbPictureBufferDesc* input_ptr);
```
Top-level ME for one 64x64 block. Orchestrates HME, integer search, pruning, and candidate construction.

```c
uint32_t svt_aom_compute8x4_sad_kernel_c(
    uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride);
```
Compute SAD for an 8x4 block. The fundamental building block for sub-sampled SAD computation.

```c
void svt_ext_all_sad_calculation_8x8_16x16_c(
    uint8_t* src, uint32_t src_stride, uint8_t* ref, uint32_t ref_stride,
    uint32_t mv, uint32_t* p_best_sad_8x8, uint32_t* p_best_sad_16x16,
    uint32_t* p_best_mv8x8, uint32_t* p_best_mv16x16,
    uint32_t p_eight_sad16x16[16][8], uint32_t p_eight_sad8x8[64][8], bool sub_sad);
```
Compute SAD for all 64 8x8 and 16 16x16 blocks within a 64x64 superblock, at 8 consecutive horizontal positions.

```c
void svt_ext_eight_sad_calculation_32x32_64x64_c(
    const uint32_t p_sad16x16[16][8], uint32_t* p_best_sad_32x32,
    uint32_t* p_best_sad_64x64, uint32_t* p_best_mv32x32,
    uint32_t* p_best_mv64x64, uint32_t mv, uint32_t p_sad32x32[4][8]);
```
Aggregate 16x16 SADs into 32x32 and 64x64 SADs for 8 search positions.

```c
void svt_aom_downsample_2d_c(
    uint8_t* input_samples, uint32_t input_stride, uint32_t input_area_width,
    uint32_t input_area_height, uint8_t* decim_samples, uint32_t decim_stride,
    uint32_t decim_step);
```
2D decimation of an image by `decim_step` in each dimension.

```c
uint16_t svt_aom_get_scaled_picture_distance(uint16_t dist);
```
Scale temporal distance to slow search region growth: `return (dist * 5) / 8 + round_up`.

### av1me.c

```c
void svt_av1_init3smotion_compensation(SearchSiteConfig* cfg, int stride);
```
Initialize 3-step diamond search pattern: generates 8 search sites per step, from MAX_FIRST_STEP (1024) down to 1.

```c
void svt_av1_set_mv_search_range(MvLimits* mv_limits, const Mv* mv);
```
Set MV search range limits based on reference MV position. Range is [-1023, 1023] full-pel.

```c
int svt_av1_full_pixel_search(
    PictureControlSet* pcs, IntraBcContext* x, BlockSize bsize,
    Mv* mvp_full, int step_param, int error_per_bit,
    int* cost_list, const Mv* ref_mv);
```
Full-pixel diamond search for IntraBC. Uses the 3-step search pattern from SearchSiteConfig.

```c
int svt_aom_mv_err_cost(const Mv* mv, const Mv* ref,
    const int* mvjcost, const int* mvcost[2], int error_per_bit);
```
Compute entropy-based MV cost.

```c
int svt_aom_mv_err_cost_light(const Mv* mv, const Mv* ref);
```
Fast approximation: `1296 + 50 * (|dx| + |dy|)`.

### mcomp.c

```c
int svt_av1_find_best_sub_pixel_tree(
    void* ictx, MacroBlockD* xd, const AV1Common* cm,
    SUBPEL_MOTION_SEARCH_PARAMS* ms_params, Mv start_mv, Mv* bestmv,
    int* distortion, unsigned int* sse1, int qp, BlockSize bsize,
    uint8_t is_intra_bordered);
```
Full-quality sub-pixel refinement: iterative 4-cardinal + diagonal search at each sub-pel level.

```c
int svt_av1_find_best_sub_pixel_tree_pruned(
    void* ictx, MacroBlockD* xd, const AV1Common* cm,
    SUBPEL_MOTION_SEARCH_PARAMS* ms_params, Mv start_mv, Mv* bestmv,
    int* distortion, unsigned int* sse1, int qp, BlockSize bsize,
    uint8_t early_neigh_check_exit);
```
Speed-optimized sub-pixel refinement: uses bilinear estimation for first-level checks, chess-pattern second-level, early exits based on error deviation.

```c
int svt_aom_fp_mv_err_cost(const Mv* mv, const svt_mv_cost_param* mv_cost_params);
```
Compute MV cost using the full parameter struct (dispatches to the appropriate cost type).

### global_me.c

```c
void svt_aom_global_motion_estimation(
    PictureParentControlSet* pcs, EbPictureBufferDesc* input_pic);
```
Top-level GM estimation. For each reference frame: detect corners, compute correspondences, run RANSAC, refine model parameters, validate error advantage.

```c
void svt_aom_upscale_wm_params(WarpedMotionParams* wm_params, uint8_t scale_factor);
```
Upscale translation parameters when detection was done on downsampled frames.

```c
void svt_aom_gm_pre_processor(
    PictureParentControlSet* pcs, PictureParentControlSet** pcs_list);
```
Pre-processing pass during temporal filtering to detect GM activity. If any non-IDENTITY GM is found, sets `gm_pp_detected = true` for the main GM pass.

### global_motion.c

```c
int64_t svt_av1_refine_integerized_param(
    GmControls* gm_ctrls, WarpedMotionParams* wm, TransformationType wmtype,
    uint8_t* ref, int r_width, int r_height, int r_stride,
    uint8_t* dst, int d_width, int d_height, int d_stride,
    int n_refinements, uint8_t chess_refn, int64_t best_frame_error,
    uint32_t pic_sad, int params_cost);
```
Iteratively refine GM parameters by testing +/- step for each parameter, using warp error as the cost function. Step starts at 16 and halves `n_refinements` times.

```c
void svt_av1_convert_model_to_params(const double* params, WarpedMotionParams* model);
```
Convert double-precision RANSAC output to integer `wmmat[]` with proper scaling, clamping, and type detection.

```c
int svt_av1_is_enough_erroradvantage(
    double best_erroradvantage, int params_cost, int erroradv_type);
```
Check if the warp error improvement justifies the parameter coding cost.

```c
void gm_compute_correspondence(
    PictureParentControlSet* pcs, uint8_t* frm_buffer, int frm_width, int frm_height,
    int frm_stride, int* frm_corners, int num_frm_corners, uint8_t* ref,
    int ref_stride, Correspondence* correspondences, int* num_correspondences,
    uint8_t list_idx, uint8_t ref_idx);
```
Dispatch to either corner-based or MV-based correspondence generation.

```c
void determine_gm_params(
    TransformationType type, MotionModel* params_by_motion, int num_motions,
    Correspondence* correspondences, int num_correspondences);
```
Run RANSAC on correspondences to find motion model parameters.

### ransac.c

```c
bool svt_aom_ransac(
    const Correspondence* matched_points, int npoints, TransformationType type,
    MotionModel* motion_models, int num_desired_motions, bool* mem_alloc_failed);
```
RANSAC model fitting. 20 random trials, 5 refinement iterations per best model. Minimum inlier fraction = 0.1. Inlier threshold = 1.25 pixels.

### corner_detect.c

```c
int svt_av1_fast_corner_detect(
    unsigned char* buf, int width, int height, int stride,
    int* points, int max_points);
```
FAST-9 corner detection with non-maximum suppression. Barrier = 18. Returns up to `max_points` corners as interleaved (x,y) pairs.

### corner_match.c

```c
int svt_av1_determine_correspondence(
    uint8_t* frm, int* frm_corners, int num_frm_corners, uint8_t* ref,
    int* ref_corners, int num_ref_corners, int width, int height,
    int frm_stride, int ref_stride, Correspondence* correspondences,
    uint8_t match_sz);
```
Find matching corners between source and reference using NCC. Match window = `match_sz x match_sz` (typically 5 or 7). NCC threshold = 0.75. Distance threshold = `(max(w,h)/16)^2`.

```c
double svt_av1_compute_cross_correlation_c(
    unsigned char* im1, int stride1, int x1, int y1,
    unsigned char* im2, int stride2, int x2, int y2, uint8_t match_sz);
```
Compute `cov^2 / var2` for NCC computation. Returns 0 if correlation is negative.

### hash_motion.c

```c
void svt_av1_generate_block_2x2_hash_value(
    const Yv12BufferConfig* picture, uint32_t* pic_block_hash,
    PictureControlSet* pcs);
```
Generate 2x2 base hash values for the entire picture.

```c
void svt_av1_generate_block_hash_value(
    const Yv12BufferConfig* picture, int block_size, uint32_t* src_pic_block_hash,
    uint32_t* dst_pic_block_hash, PictureControlSet* pcs);
```
Hierarchically combine sub-block hashes using CRC32C to produce hashes for larger block sizes.

```c
void svt_av1_get_block_hash_value(
    uint8_t* y_src, int stride, int block_size, uint32_t* hash_value1,
    uint32_t* hash_value2, int use_highbitdepth, PictureControlSet* pcs,
    IntraBcContext* x);
```
Compute the hash for a single block at the current encoding position. Uses per-block hash buffers in IntraBcContext.

### warped_motion.c / enc_warped_motion.c

```c
void svt_av1_warp_plane(
    WarpedMotionParams* wm, int use_hbd, int bd, const uint8_t* ref,
    const uint8_t* ref_2b, int width, int height, int stride, uint8_t* pred,
    int p_col, int p_row, int p_width, int p_height, int p_stride,
    int subsampling_x, int subsampling_y, ConvolveParams* conv_params);
```
Warp a rectangular region of the reference frame according to the affine model. Uses 8-tap warped interpolation filters at 1/64-pel precision.

```c
bool svt_find_projection(
    int np, int* pts1, int* pts2, BlockSize bsize, const Mv mv,
    WarpedMotionParams* wm_params, int mi_row, int mi_col);
```
Estimate local warp parameters from `np` sample points. Returns false if fitting fails.

```c
int64_t svt_av1_warp_error(
    WarpedMotionParams* wm, const uint8_t* ref, int width, int height, int stride,
    uint8_t* dst, int p_col, int p_row, int p_width, int p_height, int p_stride,
    int subsampling_x, int subsampling_y, uint8_t chess_refn, int64_t best_error);
```
Compute SAD between warped reference and source. Processes in 32x32 tiles with early termination. Chess refinement mode halves computation.

### adaptive_mv_pred.c

```c
void svt_aom_generate_av1_mvp_table(
    ModeDecisionContext* ctx, BlkStruct* blk_ptr, const BlockGeom* blk_geom,
    uint16_t blk_org_x, uint16_t blk_org_y, MvReferenceFrame* ref_frames,
    uint32_t tot_refs, PictureControlSet* pcs);
```
Build the MV prediction candidate table by scanning spatial and temporal neighbors. Populates `ref_mv_stack` with weighted MV candidates.

```c
void svt_aom_get_av1_mv_pred_drl(
    ModeDecisionContext* ctx, BlkStruct* blk_ptr, MvReferenceFrame ref_frame,
    uint8_t is_compound, PredictionMode mode, uint8_t drl_index,
    Mv nearestmv[2], Mv nearmv[2], Mv ref_mv[2]);
```
Extract the MV for a specific DRL index and prediction mode from the MV stack.

```c
bool svt_aom_warped_motion_parameters(
    ModeDecisionContext* ctx, const Mv mv, const BlockGeom* blk_geom,
    const MvReferenceFrame ref_frame, WarpedMotionParams* wm_params,
    uint8_t* num_samples, uint16_t lower_band_th, uint16_t upper_band_th,
    bool shut_approx);
```
Estimate local warped motion parameters for a block using neighbor MVs as samples.

```c
Mv svt_aom_gm_get_motion_vector_enc(
    const WarpedMotionParams* gm, int32_t allow_hp, BlockSize bsize,
    int32_t mi_col, int32_t mi_row, int32_t is_integer);
```
Derive the block-level MV from global motion parameters for a specific block position.

### global_me_cost.c

```c
int svt_aom_gm_get_params_cost(
    const WarpedMotionParams* gm, const WarpedMotionParams* ref_gm, int allow_hp);
```
Compute the bitstream coding cost for GM parameters using subexpfin coding. Accumulates costs for translation (0,1), rotation/zoom (2,3), and affine (4,5) parameters relative to a reference model.

### me_process.c

```c
void* svt_aom_motion_estimation_kernel(void* input_ptr);
```
Thread entry point. Receives tasks from picture decision, dispatches to ME (TASK_PAME), temporal filtering (TASK_TFME), or dynamic GOP detector (TASK_DG_DETECTOR_HME). Manages segment-level parallelism.

## Dependencies

| This Module | Depends On |
|-------------|------------|
| motion_estimation.c | `compute_sad.h` (SIMD SAD kernels), `aom_dsp_rtcd.h` (runtime dispatch), `pcs.h`/`sequence_control_set.h` (picture state), `reference_object.h` (reference management), `enc_intra_prediction.h` (open-loop intra), `lambda_rate_tables.h` (rate estimation), `transforms.h` (SATD) |
| av1me.c | `mcomp.h`, `aom_dsp_rtcd.h`, `adaptive_mv_pred.h`, `pcs.h`, `md_process.h` |
| mcomp.c | `mv.h`, `av1me.h`, `aom_dsp_rtcd.h`, `rd_cost.h` |
| global_me.c | `global_me_cost.h`, `global_motion.h`, `corner_detect.h`, `warped_motion.h`, `reference_object.h` |
| global_motion.c | `corner_detect.h`, `corner_match.h`, `ransac.h`, `enc_warped_motion.h` |
| ransac.c | `mathutils.h` (least squares), `random.h` (LCG random), `common_dsp_rtcd.h` |
| corner_match.c | `aom_dsp_rtcd.h` (cross-correlation SIMD) |
| corner_detect.c | `fast.h` (FAST-9 detector) |
| hash_motion.c | `aom_dsp_rtcd.h` (CRC32), `vector.h` (dynamic arrays) |
| warped_motion.c | `convolve.h`, `aom_dsp_rtcd.h` |
| adaptive_mv_pred.c | `entropy_coding.h`, `inter_prediction.h`, `aom_dsp_rtcd.h` |

## SIMD Functions

The following functions have SIMD-optimized variants (typically SSE2, SSE4.1, AVX2, and sometimes AVX-512):

### SAD Kernels
- `svt_aom_sadWxH` -- all standard block sizes (4x4 through 128x128)
- `svt_aom_sadWxHx4d` -- 4-reference SAD for all block sizes
- `svt_aom_compute8x4_sad_kernel` (via `compute_sad_avx2.h`)
- `svt_ext_all_sad_calculation_8x8_16x16`
- `svt_ext_eight_sad_calculation_32x32_64x64`
- `svt_ext_sad_calculation_8x8_16x16`
- `svt_ext_sad_calculation_32x32_64x64`
- `svt_nxm_sad_kernel`
- `svt_sad_loop_kernel`
- `svt_pme_sad_loop_kernel`
- `svt_aom_sad_16bit_kernel`

### Variance Functions
- `svt_aom_varianceWxH` -- all block sizes
- `svt_aom_sub_pixel_varianceWxH` -- sub-pixel variance for all block sizes
- `svt_aom_highbd_10_varianceWxH` -- 10-bit HBD variants

### Corner Matching
- `svt_av1_compute_cross_correlation` -- NCC computation (AVX2)

### Buffer Operations
- `svt_initialize_buffer_32bits` -- buffer initialization
- `svt_aom_downsample_2d` -- 2D decimation

### Transform/Distortion
- `svt_aom_satd` -- SATD computation

### Hash
- `svt_av1_get_crc32c_value` -- CRC32C hash computation

### Warped Motion
- `svt_av1_warp_affine` -- affine warp prediction (SSE4.1, AVX2)

### OBMC (Optional)
- `svt_aom_obmc_sadWxH` -- OBMC SAD
- `svt_aom_obmc_varianceWxH` -- OBMC variance
- `svt_aom_obmc_sub_pixel_varianceWxH` -- OBMC sub-pixel variance
