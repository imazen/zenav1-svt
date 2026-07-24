# Loop Filters & Restoration

## Overview

SVT-AV1 applies three post-reconstruction filtering stages in the following fixed order:

1. **Deblocking Loop Filter (DLF)** -- removes blocking artifacts at transform and prediction boundaries
2. **CDEF (Constrained Directional Enhancement Filter)** -- reduces ringing artifacts using directional filtering
3. **Loop Restoration (LR)** -- applies either Wiener or self-guided filtering per restoration unit to reduce coding artifacts

An optional fourth mechanism, **super-resolution**, rescales the frame horizontally between encoding and filtering. When enabled, the encoder downscales the luma width before encoding, and upscales after CDEF (before loop restoration).

The pipeline thread structure is: `DLF kernel` -> `CDEF kernel` -> `REST kernel`. Each kernel receives segments from the previous stage and processes them. The CDEF kernel also handles super-resolution upscaling and prepares restoration boundary lines.

## Source Files

| File | Lines | Role |
|---|---|---|
| `deblocking_common.c` / `.h` | ~890 | Filter masks, filter4/6/8/14, sharpness, filter level init |
| `deblocking_filter.c` / `.h` | ~1100 | Filter level picking, set_lpf_parameters, per-SB vert/horz application |
| `dlf_process.c` / `.h` | ~175 | DLF pipeline kernel (thread entry point) |
| `cdef.c` / `.h` | ~356 | CDEF direction finding, constrain function, filter_block, filter_fb |
| `enc_cdef.c` / `.h` | ~900+ | CDEF search (strength selection), cdef_frame application |
| `cdef_process.c` / `.h` | ~682 | CDEF pipeline kernel, super-res upscale, restoration prep |
| `restoration.c` / `.h` | ~1600+ | Wiener stripe filter, self-guided (sgrproj) core, LR unit application |
| `restoration_pick.c` / `.h` | ~2000+ | Restoration type/coefficient search (Wiener, sgrproj, switchable) |
| `rest_process.c` / `.h` | ~500+ | Restoration pipeline kernel |
| `super_res.c` / `.h` | ~324 | Super-resolution upscale/downscale with normative 8-tap filter |

## Test Coverage

| Test File | What It Tests |
|---|---|
| `DeblockTest.cc` | LBD/HBD loop filter functions (random data matching against C reference) |
| `CdefTest.cc` | CDEF block filter, find_dir, find_dir_dual, copy_rect, compute_cdef_dist (8/16-bit) |
| `RestorationPickTest.cc` | `av1_compute_stats` for Wiener coefficient derivation (LBD/HBD) |
| `selfguided_filter_test.cc` | `svt_apply_selfguided_restoration` correctness and speed |
| `wiener_convolve_test.cc` | `svt_av1_wiener_convolve_add_src` LBD/HBD correctness and speed |
| `SelfGuidedUtilTest.cc` | `svt_av1_pixel_proj_error` (LBD/HBD), `get_proj_subspace` |

## Data Structures

### Deblocking Filter

```
Av1DeblockingParameters {
    filter_length: u32,       // 0 (skip), 4, 6, 8, or 14
    lim: &[u8],              // limit threshold array (SIMD-width padded)
    mblim: &[u8],            // macroblock limit array
    hev_thr: &[u8],          // high edge variance threshold array
}

EdgeDir = VERT_EDGE (0) | HORZ_EDGE (1)

LoopFilterInfoN {
    lfthr: [LoopFilterThresh; MAX_LOOP_FILTER + 1],
    lvl: [[[[[u8; MAX_MODE_LF_DELTAS]; REF_FRAMES]; 2]; MAX_SEGMENTS]; MAX_PLANES],
    // lvl[plane][seg_id][dir][ref_frame][mode_delta_idx]
}

LoopFilterThresh {
    lim: [u8; SIMD_WIDTH],
    mblim: [u8; SIMD_WIDTH],
    hev_thr: [u8; SIMD_WIDTH],
}
```

### CDEF

```
CdefList {
    by: u8,   // 8x8 block row within 64x64 filter block
    bx: u8,   // 8x8 block column within 64x64 filter block
}

// 8 directions, each with 2 tap offsets (expressed as CDEF_BSTRIDE offsets)
eb_cdef_directions[8][2]  // padded to [12][2] for wraparound access at +/-2

// Primary filter taps: selected by (pri_strength >> coeff_shift) & 1
eb_cdef_pri_taps[2][2] = {{4, 2}, {3, 3}}

// Secondary filter taps
eb_cdef_sec_taps[2][2] = {{2, 1}, {2, 1}}
```

### Loop Restoration

```
RestorationUnitInfo {
    restoration_type: RESTORE_NONE | RESTORE_WIENER | RESTORE_SGRPROJ | RESTORE_SWITCHABLE,
    wiener_info: WienerInfo,
    sgrproj_info: SgrprojInfo,
}

WienerInfo {
    vfilter: [i16; 8],   // Vertical 7-tap symmetric filter (InterpKernel)
    hfilter: [i16; 8],   // Horizontal 7-tap symmetric filter
}

SgrprojInfo {
    ep: i32,              // Parameter set index [0..15]
    xqd: [i32; 2],       // Projection multipliers
}

SgrParamsType {
    r: [i32; 2],          // Filter radii for two passes (0 = disabled)
    s: [i32; 2],          // Precomputed scale factors
}

RestorationInfo {
    frame_restoration_type: RestorationType,
    restoration_unit_size: i32,       // 64, 128, or 256
    units_per_tile: i32,
    horz_units_per_tile: i32,
    vert_units_per_tile: i32,
    unit_info: &[RestorationUnitInfo],
    boundaries: RestorationStripeBoundaries,
}
```

### Super-Resolution

```
// 64 subpixel positions, 8 taps each
svt_av1_resize_filter_normative[64][8]: i16

RS_SUBPEL_BITS = 6
RS_SCALE_SUBPEL_BITS = 14
UPSCALE_NORMATIVE_TAPS = 8
```

## Algorithms

### 1. Deblocking Filter

#### 1.1 Filter Strength Derivation

Filter levels are derived per-plane, per-direction, per-segment, per-reference-frame, and per-mode:

1. **Base level**: `filter_level[0]` (vertical luma), `filter_level[1]` (horizontal luma), `filter_level_u`, `filter_level_v`.

2. **Delta LF adjustment** (if `delta_lf_present`):
   - If `delta_lf_multi`: use per-plane/direction delta from `sb_delta_lf[delta_lf_id_lut[plane][dir]]`
   - Otherwise: use single delta from `sb_delta_lf[0]`
   - `lvl_seg = clamp(delta_lf + base_level, 0, MAX_LOOP_FILTER)`

3. **Segment adjustment** (if active for `seg_lvl_lf_lut[plane][dir]`):
   - `lvl_seg = clamp(lvl_seg + segment_data, 0, MAX_LOOP_FILTER)`

4. **Mode/reference delta** (if `mode_ref_delta_enabled`):
   - `scale = 1 << (lvl_seg >> 5)` -- multiplier is 1 for levels 0-31, 2 for 32-63
   - `lvl_seg += ref_deltas[ref_frame] * scale`
   - For inter blocks: `lvl_seg += mode_deltas[mode_lf_lut[pred_mode]] * scale`
   - `mode_lf_lut` maps prediction modes to 0 (global/intra) or 1 (other inter)
   - Final clamp to `[0, MAX_LOOP_FILTER]`

5. **Sharpness** limits: `update_sharpness()` precomputes per-level limits:
   - `block_inside_limit = lvl >> ((sharpness > 0) + (sharpness > 4))`
   - If `sharpness > 0`: cap at `9 - sharpness`
   - Minimum of 1
   - `lim = block_inside_limit`
   - `mblim = 2 * (lvl + 2) + block_inside_limit`
   - `hev_thr = lvl >> 4`

#### 1.2 Filter Level Picking

Two methods for selecting frame-level filter strengths:

**LPF_PICK_FROM_FULL_IMAGE** (default): Binary search over filter levels [0, 63]. For each trial level, apply the loop filter to a copy of the reconstructed frame, compute SSE against the source, and pick the level with lowest SSE (biased toward lower levels).

- Search starts at the previous frame's level
- Step halves on convergence, with directional bias
- Early exit after `early_exit_convergence` consecutive non-improvements
- Luma searches vertical (dir=0) then horizontal (dir=1); chroma searches combined (dir=2)

**LPF_PICK_FROM_Q** (`pick_filter_level_by_q`): Estimate filter level from QP:
- Luma: `filt_guess = ROUND_POWER_OF_TWO(qindex * inter_frame_multiplier[resolution], 16)`
- For key frames: `filt_guess -= 4`, constrained by reference frame levels
- Chroma levels derived from luma: `filter_level_u/v = max(luma_lvl - 2 * (luma_lvl >> 4), 0)`

**ME-based skip** (`me_based_dlf_skip`): When average ME SAD across the frame is below a threshold (based on resolution and temporal layer), skip DLF entirely.

#### 1.3 Edge Detection and Filter Decision

`set_lpf_parameters()` determines whether and how to filter each edge:

1. Compute transform size along the edge direction using `get_transform_size()`
2. Check if current position is at a transform boundary: `coord & (tx_size - 1) == 0`
3. If at a boundary, get filter levels for both current and previous blocks
4. Skip if both blocks are skipped inter blocks (unless also a PU boundary)
5. Select filter length from minimum transform size of the two blocks:
   - `min_ts == TX_4X4` -> **4-tap** filter
   - Chroma plane -> **6-tap** filter
   - `min_ts == TX_8X8` -> **8-tap** filter
   - Otherwise -> **14-tap** filter

#### 1.4 Filter Masks

All filter decisions use a mask pattern where any violation sets the mask to "no filter":

**filter_mask** (decides whether to apply any filter):
```
mask = 0
for each adjacent pair within [p3..p0, q0..q3]:
    mask |= (|diff| > limit) * -1
mask |= (|p0-q0|*2 + |p1-q1|/2 > blimit) * -1
return ~mask   // all-ones if should filter, all-zeros if not
```

**flat_mask4** (decides whether the region is flat enough for wide filter):
```
for p1,p2,p3,q1,q2,q3 relative to p0/q0:
    mask |= (|sample - p0_or_q0| > 1) * -1
return ~mask
```

**hev_mask** (high edge variance -- decides tap structure):
```
hev = (|p1-p0| > thresh) | (|q1-q0| > thresh)
```

High bit depth variants shift thresholds by `(bd - 8)`.

#### 1.5 Filter Kernels

**filter4** (4-tap, narrow filter):
- Convert to signed domain by XOR with 0x80
- `filter = clamp(p1 - q1) & hev` (outer taps only when HEV)
- `filter = clamp(filter + 3*(q0 - p0)) & mask`
- `filter1 = clamp(filter + 4) >> 3`
- `filter2 = clamp(filter + 3) >> 3`
- `q0 -= filter1; p0 += filter2`
- Outer: `adj = round(filter1 / 2) & ~hev; q1 -= adj; p1 += adj`

**filter6** (6-tap, chroma): If flat and mask pass, apply 5-tap smoothing filter `[1,2,2,2,1]/8`:
```
op1 = round((p2*3 + p1*2 + p0*2 + q0) / 8)
op0 = round((p2 + p1*2 + p0*2 + q0*2 + q1) / 8)
oq0 = round((p1 + p0*2 + q0*2 + q1*2 + q2) / 8)
oq1 = round((p0 + q0*2 + q1*2 + q2*3) / 8)
```
Otherwise falls back to filter4.

**filter8** (8-tap): If flat and mask pass, apply 7-tap smoothing `[1,1,1,2,1,1,1]/8`:
```
op2 = round((p3*3 + p2*2 + p1 + p0 + q0) / 8)
op1 = round((p3*2 + p2 + p1*2 + p0 + q0 + q1) / 8)
...  (symmetric sliding window)
```
Otherwise falls back to filter4.

**filter14** (14-tap): If flat, flat2, and mask all pass, apply 13-tap smoothing over `[p6..p0, q0..q6]` with divisor 16:
```
op5 = round((p6*7 + p5*2 + p4*2 + p3 + p2 + p1 + p0 + q0) / 16)
...  (13-tap sliding window with weights summing to 16)
```
Otherwise falls back to filter8.

#### 1.6 Application Order

Vertical edges are filtered first, then horizontal edges. When `combine_vert_horz_lf` is enabled (always set to 1), the current SB's vertical edges are filtered, then the *previous* SB's horizontal edges are filtered (allowing vertical results to feed into horizontal filtering).

### 2. CDEF (Constrained Directional Enhancement Filter)

#### 2.1 Direction Detection

`cdef_find_dir()` detects the dominant direction of an 8x8 block among 8 directions (0=45deg up-right, 2=horizontal, 4=45deg down-right, 6=vertical):

1. For each pixel in the 8x8 block, accumulate into 8 partial sum arrays along the 8 directions. Pixels are normalized: `x = (pixel >> coeff_shift) - 128`.

2. Compute variance cost for each direction:
   ```
   cost[d] = sum over lines_in_direction(partial[d][line]^2 * div_table[line_length])
   ```
   Where `div_table = [0, 840, 420, 280, 210, 168, 140, 120, 105]` (= `840/n` for normalization).

3. Best direction = argmax(cost). Variance = `(best_cost - cost[(best_dir + 4) & 7]) >> 10`.

Direction pairs (processed two at a time by `cdef_find_dir_dual`).

#### 2.2 Primary and Secondary Filter Taps

The CDEF filter processes each pixel using primary (along-direction) and secondary (cross-direction) taps:

**Primary taps** at distance k=0,1 along direction `dir`:
- Tap weights selected by `(pri_strength >> coeff_shift) & 1`:
  - Set 0: `{4, 2}` (even strengths)
  - Set 1: `{3, 3}` (odd strengths)
- Samples: `p0 = in[pos + directions[dir][k]]`, `p1 = in[pos - directions[dir][k]]`

**Secondary taps** at distance k=0,1 along directions `dir+2` and `dir-2`:
- Tap weights: always `{2, 1}`
- Four samples per k: `s0,s1` along `dir+2`, `s2,s3` along `dir-2`

**Constrain function** (non-linear clamping):
```
constrain(diff, threshold, damping):
    if threshold == 0: return 0
    shift = max(0, damping - msb(threshold))
    return sign(diff) * min(|diff|, max(0, threshold - (|diff| >> shift)))
```

**Filter application**:
```
sum = 0
for k in 0..1:
    sum += pri_taps[k] * constrain(p0 - x, pri_strength, pri_damping)
    sum += pri_taps[k] * constrain(p1 - x, pri_strength, pri_damping)
    sum += sec_taps[k] * constrain(s0 - x, sec_strength, sec_damping)
    ... (for all 4 secondary samples)
    min/max tracking of all neighbor pixels (clamping output)

y = clamp(x + round_to_zero((8 + sum) >> 4), min, max)
```

The min/max clamping ensures CDEF never pushes a pixel beyond the range of its neighbors (the "constrained" part).

#### 2.3 Strength Selection

Strengths are encoded per 64x64 filter block. Each block gets an index into `cdef_y_strength[]` and `cdef_uv_strength[]` tables (up to 8 entries, selected by `cdef_bits`).

Each strength value encodes: `pri_strength * CDEF_SEC_STRENGTHS + sec_strength` where:
- `pri_strength` in [0, 15] (4 bits)
- `sec_strength` in [0, 3], but value 3 is mapped to 4: `sec_strength += (sec_strength == 3)`

**Search procedure** (in `cdef_seg_search`):

1. Build list of non-skip 8x8 blocks within each 64x64 filter block
2. Copy reconstruction to 16-bit padded buffer with CDEF_VERY_LARGE borders
3. For each candidate strength pair:
   - Apply `svt_cdef_filter_fb()` to temporary buffer
   - Compute MSE against source
4. Two-pass search: first pass over primary strengths, second pass over secondary strengths
5. `finish_cdef_search()` selects best strengths per filter block

**Damping**: `pri_damping = sec_damping = 3 + (base_qindex >> 6)`

**Luma strength adjustment**: For luma, primary strength is modulated by variance: `adjust_strength(strength, var) = var ? (strength * (4 + min(msb(var >> 6), 12)) + 8) >> 4 : 0`

#### 2.4 Chroma Direction Conversion

For non-4:2:0 subsampling (4:2:2 or 4:4:0), chroma direction indices are remapped:
- 4:2:2: `conv422[8] = {7, 0, 2, 4, 5, 6, 6, 6}`
- 4:4:0: `conv440[8] = {1, 2, 2, 2, 3, 4, 6, 0}`

### 3. Loop Restoration

#### 3.1 Restoration Unit Geometry

- Processing unit size: 64x64 pixels (`RESTORATION_PROC_UNIT_SIZE`)
- Restoration unit sizes: 64, 128, or 256 (signaled per plane)
- Units per tile: `max((tile_size + unit_size/2) / unit_size, 1)` -- round to nearest, not ceiling
- Grid offset: shifted 8 pixels upward from SB grid (`RESTORATION_UNIT_OFFSET = 8`)

Stripe processing uses 64-pixel-high horizontal stripes with boundary lines saved/restored around each stripe to handle inter-stripe dependencies.

#### 3.2 Wiener Filter

A 7-tap (luma) or 5-tap (chroma) separable symmetric filter:

**Coefficients**: 3 free parameters per dimension (symmetry constrains the other 3; center tap = 128 - 2*(tap0+tap1+tap2)):
- `tap0`: center 3, range [-5, 10], 4 bits
- `tap1`: center -7, range [-23, 8], 5 bits
- `tap2`: center 15, range [-17, 46], 6 bits
- `tap3` (center): computed as `WIENER_FILT_STEP - 2*(tap0 + tap1 + tap2)`

**Application** (`wiener_filter_stripe`): Separable convolution using `svt_av1_wiener_convolve_add_src()`, which applies horizontal then vertical filtering in a single pass. Processing is done in 64-pixel-wide chunks.

**Coefficient derivation** (in `restoration_pick.c`): Iterative Wiener-Hopf solution over `NUM_WIENER_ITERS = 5` iterations, alternating between horizontal and vertical filter optimization. Each iteration solves for the optimal linear filter given the current cross-filter using autocorrelation statistics (`av1_compute_stats`).

#### 3.3 Self-Guided Filter (sgrproj)

Two-pass guided filter using box sums:

**Parameters**: 16 preset parameter sets (`svt_aom_eb_sgr_params[0..15]`), each specifying:
- Pass 0: radius `r[0]` (0 or 2), scale `s[0]`
- Pass 1: radius `r[1]` (0 or 1), scale `s[1]`
- Radius 0 disables that pass

**Core algorithm** (`selfguided_restoration_internal`):

1. **Box sums**: Compute windowed sum `B[k]` and sum-of-squares `A[k]` over `(2r+1) x (2r+1)` window using separable box filters (`boxsum1` for r=1, `boxsum2` for r=2).

2. **Per-pixel statistics** (for each pixel in the extended region `[-1, height+1) x [-1, width+1)`):
   ```
   n = (2*r + 1)^2
   a = round(A[k] / 2^(2*(bd-8)))   // normalized sum of squares
   b = round(B[k] / 2^(bd-8))       // normalized sum
   p = max(a*n - b*b, 0)            // variance * n (Popoviciu bound)
   z = round(p * s / 2^20)          // scaled variance
   A[k] = x_by_xplus1[min(z, 255)] // blend factor in [1, 256]
   B[k] = round((256 - A[k]) * B[k] * one_by_x[n-1] / 2^12)
   ```

3. **Weighted combination** (for each output pixel):
   ```
   a = weighted_sum_of_neighbors(A, cross_weights=4, corner_weights=3)
   b = weighted_sum_of_neighbors(B, cross_weights=4, corner_weights=3)
   v = a * dgd[pixel] + b
   dst = round(v / 2^(SGR_BITS + 5 - RST_BITS))
   ```

4. **Fast variant** (`selfguided_restoration_fast_internal`): Only computes A/B for even rows. Even rows use above/below neighbors (weights 6/5), odd rows use left/right only (weights 6/5). Used when `r == 2`.

**Projection** (`svt_apply_selfguided_restoration`): Combines the two filter passes:
```
decode_xq(xqd, xq, params):
    if r[0] == 0: xq = [0, 128 - xqd[1]]
    if r[1] == 0: xq = [xqd[0], 0]
    else: xq = [xqd[0], 128 - xqd[0] - xqd[1]]

result = clamp(
    src + round((xq[0] * (flt0 - src*2^RST_BITS) + xq[1] * (flt1 - src*2^RST_BITS)) / 2^(PRJ_BITS + RST_BITS)),
    0, max_pixel
)
```

**Projection parameter ranges**:
- `xqd[0]`: [-96, 31] (SGRPROJ_PRJ_BITS = 7)
- `xqd[1]`: [-32, 95]

#### 3.4 Restoration Unit Selection

`restoration_seg_search()` evaluates each restoration unit:

1. Compute **no-filter SSE** (RESTORE_NONE)
2. If Wiener enabled: search for optimal Wiener coefficients via iterative Wiener-Hopf, compute Wiener SSE
3. If sgrproj enabled: search over all 16 parameter sets, solve for optimal projection coefficients, compute sgrproj SSE
4. For each frame-level restoration type (WIENER, SGRPROJ, SWITCHABLE):
   - Pick per-unit best type using RD cost: `cost = SSE * (1 << RDDIV_BITS) + rate * rdmult / (1 << AV1_PROB_COST_SHIFT)`

`rest_finish_search()` selects the frame-level restoration type (per plane) that minimizes total RD cost.

#### 3.5 Stripe Boundary Management

Loop restoration uses striped processing to handle tile boundaries:

- Before filtering each stripe, save 3 border rows above and below the stripe
- Replace them with saved deblocked or CDEF boundary lines (`stripe_boundary_above`, `stripe_boundary_below`)
- After filtering the stripe, restore the original border pixels
- Boundary lines are saved at two points: before CDEF (deblocked) and after CDEF
- The `optimized_lr` flag enables a fast path that only saves/restores the outermost border row

### 4. Super-Resolution

#### 4.1 Downscale

The encoder reduces `frame_width` by a scale denominator (9-16, where 8 = no scaling):
```
scaled_width = (original_width * 8 + denom/2) / denom
```
Special case: denominator 17 means 3/4 scaling: `scaled = (3 + width*3) >> 2`.

Minimum dimension after scaling: `max(16, original_dim)`.

#### 4.2 Upscale

`svt_av1_upscale_normative_rows()` applies horizontal-only upscaling using an 8-tap polyphase filter:

1. **Step calculation**:
   ```
   x_step_qn = (in_length << 14 + out_length/2) / out_length
   x0_qn = initial fractional offset (centered)
   ```

2. **Per-pixel convolution**:
   ```
   for each output pixel x:
       src_x = &src[x_qn >> 14]
       filter_idx = (x_qn & 0x3FFF) >> 8   // 6-bit subpixel index
       filter = resize_filter_normative[filter_idx]
       sum = 0
       for k in 0..7: sum += src_x[k] * filter[k]
       dst[x] = clip(round(sum / 128))
       x_qn += x_step_qn
   ```

3. **Border handling**: Left/right frame edges are extended by replicating the edge pixel for `UPSCALE_NORMATIVE_TAPS/2 + 1 = 5` columns.

4. **Tile column processing**: Each tile column is upscaled independently, with the fractional position state carried across tile boundaries.

#### 4.3 Integration

Super-resolution is applied in the CDEF kernel after CDEF filtering completes:
1. Copy the reconstructed frame
2. Upscale each plane from `frame_width` to `superres_upscaled_width`
3. Loop restoration then operates on the upscaled frame

## Key Functions

### Deblocking Filter

| Function | Description |
|---|---|
| `svt_av1_loop_filter_init()` | Initialize filter limits, sharpness, hev thresholds |
| `svt_av1_loop_filter_frame_init()` | Compute per-segment per-ref per-mode filter levels |
| `svt_aom_update_sharpness()` | Compute `lim` and `mblim` arrays from sharpness level |
| `set_lpf_parameters()` | Determine filter_length and thresholds for one edge |
| `svt_av1_filter_block_plane_vert()` | Filter all vertical edges in one SB |
| `svt_av1_filter_block_plane_horz()` | Filter all horizontal edges in one SB |
| `svt_aom_loop_filter_sb()` | Filter one SB (orchestrates vert then horz) |
| `svt_av1_loop_filter_frame()` | Filter entire frame (iterates over all SBs) |
| `svt_av1_pick_filter_level()` | Search for optimal filter level (full image method) |
| `svt_av1_pick_filter_level_by_q()` | Estimate filter level from QP |
| `svt_aom_get_filter_level_delta_lf()` | Get per-block filter level with delta LF |

### CDEF

| Function | Description |
|---|---|
| `svt_aom_cdef_find_dir()` | Find dominant direction for one 8x8 block |
| `svt_aom_cdef_find_dir_dual()` | Find direction for two 8x8 blocks simultaneously |
| `svt_cdef_filter_block()` | Apply CDEF filter to one block |
| `svt_cdef_filter_fb()` | Filter all non-skip blocks in one 64x64 filter block |
| `svt_av1_cdef_frame()` | Apply CDEF to entire frame |
| `finish_cdef_search()` | Select final CDEF strengths from search results |
| `svt_sb_compute_cdef_list()` | Build list of non-skip 8x8 blocks |
| `svt_aom_copy_sb8_16()` | Copy 8-bit or 16-bit data to 16-bit CDEF buffer |

### Loop Restoration

| Function | Description |
|---|---|
| `svt_av1_loop_restoration_filter_unit()` | Filter one restoration unit |
| `svt_av1_loop_restoration_filter_frame()` | Filter entire frame (called from rest kernel) |
| `svt_av1_selfguided_restoration()` | Core self-guided filter (computes flt0, flt1) |
| `svt_apply_selfguided_restoration()` | Apply sgrproj with projection coefficients |
| `svt_av1_wiener_convolve_add_src()` | Wiener separable convolution |
| `svt_av1_loop_restoration_save_boundary_lines()` | Save stripe boundaries for later restoration |
| `restoration_seg_search()` | Per-segment restoration type/coefficient search |
| `rest_finish_search()` | Select frame-level restoration types |
| `svt_av1_alloc_restoration_struct()` | Allocate restoration unit info arrays |
| `svt_aom_foreach_rest_unit_in_frame()` | Iterate visitor over all restoration units |

### Super-Resolution

| Function | Description |
|---|---|
| `svt_av1_upscale_normative_rows()` | Upscale rows using 8-tap normative filter |
| `calculate_scaled_size_helper()` | Compute downscaled dimension |
| `svt_av1_superres_upscale_frame()` | Full frame upscale (in cdef_process.c) |

## Dependencies

### Upstream (required inputs)

- **Reconstructed frame** from enc/dec process
- **Mode info grid** (`mi_grid_base`): block sizes, prediction modes, reference frames, skip flags, transform depths
- **QP / quantization parameters**: base_q_idx drives CDEF damping and DLF level estimation
- **Frame header**: loop filter params, CDEF params, restoration info, segmentation, delta LF
- **Source frame**: used for SSE computation during filter level search
- **Reference frame filter state**: previous frame filter levels for search initialization

### Downstream (outputs consumed by)

- **Entropy coding**: CDEF strength indices, restoration type/coefficients, filter levels
- **Reference frame storage**: filtered reconstruction becomes reference for future frames
- **Recon output**: final filtered frame for display/output

### Internal dependencies between stages

```
DLF output -> save boundary lines (pre-CDEF) -> CDEF input
CDEF output -> save boundary lines (post-CDEF) -> super-res upscale -> LR input
LR output -> final reconstruction
```

## SIMD Functions

### Deblocking Filter

| Function | SSE2 | AVX2 | NEON |
|---|---|---|---|
| `svt_aom_lpf_horizontal_4` | Yes | -- | Yes |
| `svt_aom_lpf_horizontal_6` | Yes | -- | Yes |
| `svt_aom_lpf_horizontal_8` | Yes | -- | Yes |
| `svt_aom_lpf_horizontal_14` | Yes | -- | Yes |
| `svt_aom_lpf_vertical_4` | Yes | -- | Yes |
| `svt_aom_lpf_vertical_6` | Yes | -- | Yes |
| `svt_aom_lpf_vertical_8` | Yes | -- | Yes |
| `svt_aom_lpf_vertical_14` | Yes | -- | Yes |
| `svt_aom_highbd_lpf_horizontal_*` | SSE2 | -- | NEON |
| `svt_aom_highbd_lpf_vertical_*` | SSE2 | -- | NEON |

### CDEF

| Function | AVX2 | AVX-512 | NEON |
|---|---|---|---|
| `svt_cdef_filter_block` | Yes | -- | Yes |
| `svt_cdef_filter_block_8xn_16` | Yes | Yes | -- |
| `svt_aom_cdef_find_dir` | AVX2 | -- | NEON |
| `svt_aom_cdef_find_dir_dual` | AVX2 | -- | NEON |
| `svt_aom_copy_rect8_8bit_to_16bit` | SSE4.1 | AVX2 | NEON |
| `svt_compute_cdef_dist_16bit` | SSE4.1 | AVX2/512 | NEON |
| `svt_compute_cdef_dist_8bit` | SSE4.1 | AVX2/512 | NEON |

### Loop Restoration

| Function | SSE2/4.1 | AVX2 | AVX-512 | NEON |
|---|---|---|---|---|
| `svt_av1_wiener_convolve_add_src` | SSE2 | AVX2 | AVX-512 | NEON |
| `svt_av1_selfguided_restoration` | SSE4.1 | AVX2 | -- | NEON |
| `svt_apply_selfguided_restoration` | SSE4.1 | AVX2 | -- | -- |
| `svt_av1_compute_stats` | SSE4.1 | AVX2 | AVX-512 | NEON |
| `svt_av1_compute_stats_highbd` | SSE4.1 | AVX2 | AVX-512 | NEON |
| `svt_av1_pixel_proj_error` | SSE4.1 | AVX2 | -- | -- |

### Constants

Key lookup tables that must be ported exactly:

- `svt_aom_eb_sgr_params[16]`: Self-guided filter parameter presets
- `svt_aom_eb_x_by_xplus1[256]`: `round(256 * x / (x + 1))` LUT
- `svt_aom_eb_one_by_x[25]`: `round(4096 / x)` LUT
- `eb_cdef_directions_padded[12][2]`: Direction offsets with wraparound padding
- `svt_aom_eb_cdef_pri_taps[2][2]`, `svt_aom_eb_cdef_sec_taps[2][2]`: Filter tap weights
- `svt_av1_resize_filter_normative[64][8]`: Super-resolution interpolation filter bank
- `mode_lf_lut[25]`: Prediction mode to deblocking mode class mapping
- `div_table[9]`: CDEF direction cost normalization factors
