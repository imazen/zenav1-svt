# Film Grain Synthesis

## Overview

The film grain system in SVT-AV1 has two distinct phases:

1. **Noise Model Estimation (encoder-side):** Analyze input video to extract film grain parameters. This involves denoising the input with a Wiener filter, modeling the residual noise using an auto-regressive (AR) model, and fitting noise strength as a function of intensity. The result is a quantized `AomFilmGrain` parameter set written into the bitstream.

2. **Grain Synthesis (decoder-side):** Given the `AomFilmGrain` parameters, generate pseudo-random grain and add it to the reconstructed frame. This process is normative -- the encoder's reference implementation must produce bit-exact output matching the decoder.

The encoder can operate in two modes: **adaptive film grain** (re-estimate parameters per frame) or **user-supplied grain tables**. Both converge on an `AomFilmGrain` struct that is signaled in the frame header.

## Source Files

| File | Role | LOC (approx) |
|------|------|-------------|
| `Source/Lib/Codec/grainSynthesis.c` | Normative grain synthesis (decoder-side algorithm) | ~1300 |
| `Source/Lib/Codec/grainSynthesis.h` | Synthesis function declarations | ~50 |
| `Source/Lib/Codec/noise_model.c` | Noise estimation, AR fitting, Wiener denoising | ~2400 |
| `Source/Lib/Codec/noise_model.h` | Noise model data structures and API | ~370 |
| `Source/Lib/Codec/noise_util.c` | FFT-based noise transform utilities | ~370 |
| `Source/Lib/Codec/noise_util.h` | Noise transform declarations | ~60 |
| `Source/API/EbSvtAv1.h` | `AomFilmGrain` struct definition (lines 307-381) | -- |

## Test Coverage

| Test File | Framework | What It Tests |
|-----------|-----------|--------------|
| `test/FilmGrainTest.cc` | Google Test | Synthesis correctness against 3 hardcoded test vectors with known expected output; tests multiple AR lag values and scaling point configurations |
| `test/noise_model_test.cc` | Google Test | AVX2 vs C equivalence for: `add_block_observations_internal`, `pointwise_multiply`, `noise_tx_filter`, `flat_block_finder_extract_block` |

## Data Structures

### AomFilmGrain (Source/API/EbSvtAv1.h:307-381)

The complete bitstream-level parameter set for one frame's grain:

| Field | Type | Bits | Description |
|-------|------|------|-------------|
| `apply_grain` | i32 | 1 | Whether decoder applies grain |
| `update_parameters` | i32 | 1 | Whether to update from previous frame |
| `scaling_points_y[14][2]` | i32 | 8+8 each | Luma scaling function: (intensity, scaling_value) pairs |
| `num_y_points` | i32 | 4 | Number of luma scaling points (0..14) |
| `scaling_points_cb[10][2]` | i32 | 8+8 each | Cb scaling function points |
| `num_cb_points` | i32 | 4 | Number of Cb points (0..10) |
| `scaling_points_cr[10][2]` | i32 | 8+8 each | Cr scaling function points |
| `num_cr_points` | i32 | 4 | Number of Cr points (0..10) |
| `scaling_shift` | i32 | 2 | Right-shift for scaling (range 8..11) |
| `ar_coeff_lag` | i32 | 2 | AR coefficient lag (0..3) |
| `ar_coeffs_y[24]` | i32 | 8 each | Luma AR coefficients |
| `ar_coeffs_cb[25]` | i32 | 8 each | Cb AR coefficients (last = luma correlation) |
| `ar_coeffs_cr[25]` | i32 | 8 each | Cr AR coefficients (last = luma correlation) |
| `ar_coeff_shift` | i32 | 2 | AR coefficient precision: 6=[-2,2), 7=[-1,1), 8=[-0.5,0.5), 9=[-0.25,0.25) |
| `cb_mult` | i32 | 8 | Cb component multiplier |
| `cb_luma_mult` | i32 | 8 | Cb luma multiplier |
| `cb_offset` | i32 | 9 | Cb offset |
| `cr_mult` | i32 | 8 | Cr component multiplier |
| `cr_luma_mult` | i32 | 8 | Cr luma multiplier |
| `cr_offset` | i32 | 9 | Cr offset |
| `overlap_flag` | i32 | 1 | Whether to blend at block boundaries |
| `clip_to_restricted_range` | i32 | 1 | Clip to studio/legal range |
| `bit_depth` | i32 | -- | Video bit depth (8, 10, or 12) |
| `chroma_scaling_from_luma` | i32 | 1 | Derive chroma scaling from luma scaling |
| `grain_scale_shift` | i32 | 2 | Downscale random values during synthesis |
| `random_seed` | u16 | 16 | Per-frame PRNG seed |
| `ignore_ref` | i32 | 1 | Encoder hint: ignore reference frame map |

### AomNoiseModel (noise_model.h:181-188)

Internal encoder-side model for estimating grain across frames:

| Field | Type | Description |
|-------|------|-------------|
| `params` | `AomNoiseModelParams` | Shape (diamond/square), lag, bit_depth, use_highbd |
| `combined_state[3]` | `AomNoiseState` | Accumulated AR estimate per channel (Y, Cb, Cr) |
| `latest_state[3]` | `AomNoiseState` | Current frame's AR estimate per channel |
| `coords` | `int32_t (*)[2]` | (x, y) offsets of AR coefficient neighbors |
| `n` | i32 | Number of AR coefficients |

### AomNoiseState (noise_model.h:169-174)

Per-channel noise estimation state:

| Field | Type | Description |
|-------|------|-------------|
| `eqns` | `AomEquationSystem` | Normal equations for AR coefficient estimation (A*x = b) |
| `strength_solver` | `AomNoiseStrengthSolver` | Noise std as function of intensity |
| `num_observations` | i32 | Sample count used in current estimate |
| `ar_gain` | f64 | Gain of the AR filter (sqrt(var / noise_var)) |

### AomNoiseStrengthSolver (noise_model.h:69-76)

Models noise standard deviation as a piecewise-linear function of pixel intensity:

| Field | Type | Description |
|-------|------|-------------|
| `eqns` | `AomEquationSystem` | Bin-indexed linear system |
| `min_intensity` | f64 | 0 |
| `max_intensity` | f64 | (1 << bit_depth) - 1 |
| `num_bins` | i32 | Always 20 |
| `num_equations` | i32 | Number of (block_mean, noise_std) observations |
| `total` | f64 | Sum of all noise_std observations |

### AomFlatBlockFinder (noise_model.h:120-127)

Precomputed data for detecting flat blocks suitable for noise measurement:

| Field | Type | Description |
|-------|------|-------------|
| `at_a_inv` | f64* | Precomputed (A^T A)^{-1} for planar model (3x3) |
| `A` | f64* | Design matrix (n x 3) mapping pixel positions to [y, x, 1] |
| `num_params` | i32 | Always 3 (kLowPolyNumParams) |
| `block_size` | i32 | Typically 32 |
| `normalization` | f64 | (1 << bit_depth) - 1 |
| `use_highbd` | i32 | Whether to interpret data as uint16 |

### aom_noise_tx_t (noise_util.h:24-30)

FFT-based transform for Wiener denoising:

| Field | Type | Description |
|-------|------|-------------|
| `tx_block` | f32* | Frequency-domain buffer (2 * block_size^2 floats, 32-byte aligned) |
| `temp` | f32* | Working buffer (same size, 32-byte aligned) |
| `block_size` | i32 | 2, 4, 8, 16, or 32 |
| `fft` | fn ptr | Forward FFT function |
| `ifft` | fn ptr | Inverse FFT function |

## Algorithms

### Algorithm 1: Flat Block Detection

**Purpose:** Identify image blocks whose content is flat enough that the noise can be reliably measured. Textured or edge-containing blocks are unsuitable because their structure would corrupt the noise estimate.

**Input:** Raw frame data, block size (typically 32)

**Steps:**

1. For each non-overlapping block, extract the block and fit a low-order planar model `[y, x, 1] * [a1, a2, a3]^T` to remove smooth gradients. The planar fit uses a precomputed `(A^T A)^{-1} A^T` projection.

2. Compute gradient covariance on the residual (block minus plane):
   - `g_xx = sum(gx^2)`, `g_xy = sum(gx*gy)`, `g_yy = sum(gy^2)` over interior pixels
   - Gradients: `gx = (block[y][x+1] - block[y][x-1]) / 2`, `gy = (block[y+1][x] - block[y-1][x]) / 2`

3. Compute eigenvalues of the gradient matrix:
   - `trace = g_xx + g_yy`
   - `det = g_xx * g_yy - g_xy^2`
   - `e1 = (trace + sqrt(trace^2 - 4*det)) / 2` (spectral norm)
   - `e2 = (trace - sqrt(trace^2 - 4*det)) / 2`
   - `ratio = e1 / max(e2, 1e-6)`

4. Classify as flat if ALL of:
   - `trace < 0.15 / (32*32)`
   - `ratio < 1.25`
   - `norm (e1) < 0.08 / (32*32)`
   - `var > 0.005 / n` (must have *some* variance, otherwise it is a blank region)

5. Additionally compute a sigmoid score using learned weights:
   - `score = 1 / (1 + exp(-(w[0]*var + w[1]*ratio + w[2]*trace + w[3]*norm + w[4])))`
   - Weights: `[-6682, -0.2056, 13087, -12434, 2.5694]`

6. Take the union of threshold-classified flat blocks and the top 10th percentile by score.

### Algorithm 2: AR Model Fitting

**Purpose:** Estimate auto-regressive coefficients that describe the spatial correlation structure of the noise.

**Input:** Raw frame, denoised frame, flat block map

**AR Neighborhood:** The number and layout of coefficients depends on `lag` (1..3) and `shape` (diamond or square):

- **Diamond shape:** `lag * (lag + 1)` coefficients. For lag=3: 12 coefficients.
- **Square shape:** `((2*lag+1)^2) / 2` coefficients.

The coefficient coordinates enumerate all (x, y) offsets with y < 0 (above current pixel) plus y == 0, x < 0 (left of current pixel) -- the causal neighborhood.

**Steps:**

1. For each flat block pixel (x, y), extract the noise residual `val = data[y][x] - denoised[y][x]` and the residuals at each neighbor coordinate `buffer[i] = data[y+dy_i][x+dx_i] - denoised[y+dy_i][x+dx_i]`.

2. For chroma channels, also extract the average luma noise at the corresponding (possibly subsampled) position as an additional predictor:
   ```
   buffer[n_coeffs] = (avg_luma_data - avg_luma_denoised) / num_subsamples
   ```

3. Accumulate normal equations: `A += buffer_norm * buffer^T`, `b += buffer_norm * val`, where `buffer_norm = buffer / normalization^2`.

4. Solve `A * x = b` using Gaussian elimination (`linsolve`). The solution `x` gives the AR coefficients.

5. For chroma, if the linear system is singular, fall back to solving only for the luma correlation coefficient (last element), setting all AR coefficients to zero.

6. Compute AR gain: `ar_gain = max(1, sqrt(max(var / noise_var, 1e-6)))` where:
   - `var = mean(diagonal of A / num_observations)`
   - `noise_var = max(var - sum_covar, 1e-6)`
   - `sum_covar = sum(b_i * x_i / num_observations)` (adjusting for chroma-luma correlation)

### Algorithm 3: Noise Strength Estimation

**Purpose:** Model noise standard deviation as a function of pixel intensity, producing the scaling function points.

**Steps:**

1. Divide the intensity range `[0, (1<<bit_depth)-1]` into 20 evenly-spaced bins.

2. For each flat block, compute:
   - `block_mean`: average intensity of the block (or corresponding luma block for chroma)
   - `noise_var`: variance of the noise residual (data - denoised)

3. For chroma channels, subtract the correlated luma component:
   - `luma_strength = luma_gain * noise_strength_at(block_mean)`
   - `uncorr_std = sqrt(max(noise_var/16, noise_var - (corr * luma_strength)^2))`
   - `adjusted_strength = uncorr_std / noise_gain`

4. Add measurement `(block_mean, adjusted_strength)` to the strength solver using linear interpolation between the two nearest bins:
   - Bin index: `(num_bins - 1) * (val - min_intensity) / range`
   - Fractional part `a` distributes the observation between bins `i0` and `i1`

5. Solve the regularized linear system: add smoothness regularization `k_alpha * [-1, 2, -1]` tridiagonal penalty and a small identity term biased toward the mean.

6. Fit a piecewise-linear approximation by greedily removing points with the smallest residual until either:
   - The number of points reaches the maximum (14 for luma, 10 for chroma), or
   - The average residual per unit x exceeds tolerance `max_intensity * 0.00625 / 255.0`

### Algorithm 4: Wiener Denoising

**Purpose:** Produce a denoised version of the input frame for noise residual computation.

**Steps:**

1. Divide the frame into overlapping blocks (half-overlap) of `block_size` (default 32).

2. For each block:
   a. Extract and subtract the planar model (same as flat block finder).
   b. Multiply by a half-cosine window function.
   c. Forward FFT the windowed residual.
   d. Apply Wiener filter in frequency domain:
      - For each coefficient, compute power `p = re^2 + im^2`
      - If `p > k_beta * psd` and `p > 1e-6`: multiply by `(p - psd) / max(p, 1e-6)`
      - Otherwise: multiply by `(k_beta - 1) / k_beta` where `k_beta = 1.1`
   e. Inverse FFT.
   f. Accumulate (add) the block + plane into the output using the window weights.

3. Dither and quantize the floating-point result back to integer.

### Algorithm 5: Parameter Quantization (noise_model_get_grain_parameters)

**Purpose:** Convert floating-point AR coefficients and noise strength curves to the quantized `AomFilmGrain` format.

**Steps:**

1. Fit piecewise-linear scaling functions from the strength solver (max 14 points luma, 10 chroma).

2. Normalize scaling points to 8-bit domain (divide by `1 << (bit_depth - 8)`).

3. Choose `scaling_shift` (range 8..11) based on `max_scaling_value`:
   - `max_scaling_value_log2 = clamp(floor(log2(max_scaling_value) + 1), 2, 5)`
   - `scaling_shift = 5 + (8 - max_scaling_value_log2)`

4. Scale and round the scaling function values: `clamp(round(scale_factor * point), 0, 255)`.

5. Compute chroma-luma correlation:
   - Weighted average strength per channel
   - `y_corr[c] = avg_luma_strength * eqns.x[n_coeff] / average_strength_c`

6. Choose `ar_coeff_shift` (range 6..9):
   - `ar_coeff_shift = clamp(7 - max(1 + floor(log2(max_coeff)), ceil(log2(-min_coeff))), 6, 9)`

7. Quantize AR coefficients: `clamp(round(scale * coeff), -128, 127)`.

8. Set default chroma mixing parameters: `cb_mult=128`, `cb_luma_mult=192`, `cb_offset=256` (same for cr).

### Algorithm 6: Grain Synthesis (Normative/Decoder-Side)

**Purpose:** Generate and apply film grain to a reconstructed frame. This algorithm must be bit-exact across all implementations.

**PRNG:** 16-bit LFSR with taps at bits 0, 1, 3, 12:
```
bit = ((reg >> 0) ^ (reg >> 1) ^ (reg >> 3) ^ (reg >> 12)) & 1
reg = (reg >> 1) | (bit << 15)
result = (reg >> (16 - bits)) & ((1 << bits) - 1)
```

**PRNG Initialization per row:**
```
reg = (msb << 8) + lsb    // from random_seed
luma_num = luma_line >> 5
reg ^= ((luma_num * 37 + 178) & 255) << 8
reg ^= ((luma_num * 173 + 105) & 255)
```

**Step 1: Generate Luma Grain Block**

Block dimensions: `(top_pad + 2*ar_pad + 2*subblock_y + bottom_pad)` x `(left_pad + 2*ar_pad + 2*subblock_x + 2*ar_pad + right_pad)` where `subblock = 32`, pads = 3, `ar_pad = 3`.

1. Fill entire block with Gaussian samples: `gaussian_sequence[PRNG(11)]` shifted by `gauss_sec_shift = 12 - bit_depth + grain_scale_shift`.

2. Apply AR filter over the interior (excluding padding):
   ```
   for each interior (i, j):
       wsum = sum(ar_coeffs_y[pos] * grain[i + dy[pos]][j + dx[pos]])
       grain[i][j] = clamp(grain[i][j] + (wsum + rounding) >> ar_coeff_shift, grain_min, grain_max)
   ```
   Where `grain_center = 128 << (bit_depth - 8)`, `grain_min = -grain_center`, `grain_max = 255_shifted - grain_center`.

**Step 2: Generate Chroma Grain Blocks**

Same as luma but with subsampled dimensions. Each chroma channel uses a different PRNG seed offset (Cb: `7<<5`, Cr: `11<<5`).

For the luma correlation coefficient (last AR position, `pred_pos[2] == 1`):
```
av_luma = average of co-located luma grain samples (accounting for subsampling)
wsum += ar_coeffs_cb[pos] * av_luma
```

**Step 3: Build Scaling LUTs**

For each channel, interpolate scaling points into a 256-entry (8-bit) or appropriately scaled LUT:
```
for each interval [point[i], point[i+1]):
    delta = delta_y * (65536 + delta_x/2) / delta_x
    for x in [0, delta_x):
        lut[point[i].x + x] = point[i].y + (x * delta + 32768) >> 16
```
Extrapolate flat at both ends.

For higher bit depths, `scale_lut()` interpolates between 8-bit LUT entries:
```
x = index >> (bit_depth - 8)
frac = index & ((1 << (bit_depth - 8)) - 1)
result = lut[x] + (lut[x+1] - lut[x]) * frac + rounding) >> (bit_depth - 8)
```

**Step 4: Apply Grain to Pixels**

Process in 32x32 luma subblocks (16x16 chroma for 4:2:0). For each subblock, select a random offset into the grain template:
```
offset_y = PRNG(8) & 15
offset_x = (PRNG_result >> 4) & 15
```

For luma:
```
pixel' = clamp(pixel + (scale_lut(pixel) * grain_sample + rounding) >> scaling_shift, min, max)
```

For chroma, the scaling LUT index is a weighted blend of luma and chroma:
```
index = clamp((average_luma * cb_luma_mult + cb_mult * chroma_pixel) >> 6 + cb_offset, 0, max_range)
pixel' = clamp(chroma + (scale_lut_cb(index) * cb_grain + rounding) >> scaling_shift, min, max)
```

Where `cb_mult`, `cb_luma_mult`, `cb_offset` are the signaled parameters minus their biases (128, 128, 256 respectively).

**Step 5: Overlap Blending**

When `overlap_flag` is set, adjacent subblocks are blended at their 2-pixel-wide boundaries:

- Vertical boundary (2 columns): weights `[27,17]` and `[17,27]` (out of 32, with rounding +16)
- Horizontal boundary (2 rows): weights `[27,17]` and `[17,27]`
- Single-pixel boundary (chroma with subsampling): weights `[23,22]` (out of 32+13=45... actually `(23+22)/32` effectively)

Line buffers and column buffers cache the boundary grain values between subblocks.

### Algorithm 7: Chroma Scaling From Luma Mode

When `chroma_scaling_from_luma` is enabled:
- Copy luma scaling LUT to both chroma LUTs
- Override chroma mixing: `cb_mult=0, cb_luma_mult=64, cb_offset=0` (same for cr)
- This makes chroma grain purely a function of luma intensity

## Key Functions

### Noise Model Estimation (noise_model.c)

| Function | Description |
|----------|-------------|
| `svt_aom_denoise_and_model_run` | Top-level: denoise frame, estimate noise model, extract grain params |
| `svt_aom_flat_block_finder_init` | Precompute planar model matrices for block size |
| `svt_aom_flat_block_finder_run` | Classify blocks as flat/not-flat using gradient analysis |
| `svt_aom_flat_block_finder_extract_block_c` | Extract block, subtract planar fit |
| `svt_aom_noise_model_init` | Allocate and initialize noise model for given lag/shape |
| `noise_model_update` | Update noise model with one frame's observations |
| `add_block_observations` | Accumulate AR normal equations from flat blocks |
| `svt_av1_add_block_observations_internal_c` | Inner loop: build A and b matrices |
| `add_noise_std_observations` | Accumulate noise strength measurements per block |
| `ar_equation_system_solve` | Solve AR equations, compute AR gain |
| `svt_aom_noise_strength_solver_init` | Initialize strength solver with 20 bins |
| `svt_aom_noise_strength_solver_add_measurement` | Add one (mean, strength) observation |
| `svt_aom_noise_strength_solver_solve` | Solve regularized strength equations |
| `svt_aom_noise_strength_solver_fit_piecewise` | Reduce to piecewise-linear LUT |
| `svt_aom_noise_model_get_grain_parameters` | Quantize model to AomFilmGrain |
| `svt_aom_noise_model_save_latest` | Copy latest state to combined state |
| `is_ref_noise_model_different` | Compare two noise models |

### Wiener Denoising (noise_model.c + noise_util.c)

| Function | Description |
|----------|-------------|
| `svt_aom_wiener_denoise_2d` | Full-frame overlapped-block Wiener denoising |
| `svt_aom_noise_tx_malloc` | Allocate FFT transform context (sizes 2,4,8,16,32) |
| `svt_aom_noise_tx_forward` | Forward FFT |
| `svt_aom_noise_tx_filter_c` | Wiener filter in frequency domain |
| `svt_aom_noise_tx_inverse` | Inverse FFT with 1/n normalization |
| `svt_aom_noise_psd_get_default_value` | Default PSD: `(factor^2 / 10000) * block_size^2 / 8` |
| `svt_av1_pointwise_multiply_c` | Element-wise multiply window * (plane, block) |

### Grain Synthesis (grainSynthesis.c)

| Function | Description |
|----------|-------------|
| `svt_av1_add_film_grain_run` | Top-level: generate grain and add to frame |
| `init_arrays` | Allocate grain blocks, line/col buffers, AR position tables |
| `generate_luma_grain_block` | Fill with Gaussian noise, apply AR filter |
| `generate_chroma_grain_blocks` | Same for Cb/Cr with luma correlation |
| `init_scaling_function` | Build 256-entry scaling LUT from scaling points |
| `scale_lut` | Interpolating LUT lookup for high bit depth |
| `add_noise_to_block` | Apply grain to 8-bit pixels using scaling LUT |
| `add_noise_to_block_hbd` | Apply grain to 10/12-bit pixels |
| `ver_boundary_overlap` | Blend grain at vertical subblock boundaries |
| `hor_boundary_overlap` | Blend grain at horizontal subblock boundaries |
| `init_random_generator` | Initialize PRNG from seed and row number |
| `get_random_number` | 16-bit LFSR PRNG |
| `svt_aom_film_grain_params_equal` | Compare two AomFilmGrain structs |
| `svt_aom_fgn_copy_rect` | Copy rectangular region (handles hbd stride) |

## Dependencies

### Internal Dependencies

| Dependency | Used For |
|------------|----------|
| `mathutils.h` | `linsolve()` for solving normal equations |
| `aom_dsp_rtcd.h` | FFT function pointers (fft2x2..fft32x32), SIMD dispatch |
| `pic_buffer_desc.h` | `EbPictureBufferDesc` for frame I/O |
| `pic_operators.c` | `svt_aom_pack_2d_pic`, `svt_aom_un_pack2d` for 10-bit packing |
| `definitions.h` | `clamp()`, `AOMMIN`, `AOMMAX`, `DECLARE_ALIGNED` |

### External Dependencies

| Library | Used For |
|---------|----------|
| `<math.h>` | `sqrt`, `log2`, `floor`, `ceil`, `cos`, `exp`, `fabs`, `pow`, `round` |
| `<stdlib.h>` | `malloc`, `free`, `qsort` |
| `<string.h>` | `memset`, `memcpy`, `memmove`, `memcmp` |

### Key Constants

| Constant | Value | Meaning |
|----------|-------|---------|
| `k_max_lag` | 4 | Maximum AR lag supported by noise model |
| `kLowPolyNumParams` | 3 | Planar model parameters: [y, x, 1] |
| `k_num_bins` | 20 | Noise strength solver bins |
| `gauss_bits` | 11 | Bits used for Gaussian LUT index |
| `gaussian_sequence[2048]` | Hardcoded | 12-bit zero-mean Gaussian samples (std ~512) |
| `luma_subblock_size_{x,y}` | 32 | Grain generation subblock size |
| `chroma_subblock_size_{x,y}` | 16 | Chroma subblock (for 4:2:0) |
| AR padding | 3 | Padding for AR stabilization |

## SIMD Functions

All SIMD variants are AVX2. Dispatch is via `aom_dsp_rtcd.c`:

| Function | C Reference | AVX2 Variant | Description |
|----------|-------------|-------------|-------------|
| `svt_av1_add_block_observations_internal` | `_c` | `_avx2` | Inner loop of AR normal equation accumulation |
| `svt_av1_pointwise_multiply` | `_c` | `_avx2` | Window function * (plane, block) element-wise multiply |
| `svt_aom_noise_tx_filter` | `_c` | `_avx2` | Wiener filter in frequency domain |
| `svt_aom_flat_block_finder_extract_block` | `_c` | `_avx2` | Block extraction with planar model subtraction |

FFT functions (`svt_aom_fft{2,4,8,16,32}x{2,4,8,16,32}_float`) are also dispatched through RTCD but are shared infrastructure, not specific to film grain.

On non-x86 platforms (aarch64), all four functions fall back to C-only implementations.
