# 15. Runtime CPU Dispatch (RTCD)

This chapter documents the RTCD (Run-Time CPU Dispatch) system that allows SVT-AV1 to select optimized SIMD implementations at runtime based on detected CPU capabilities.

---

## 15.1 Overview

SVT-AV1 uses a function-pointer-based dispatch system. For every performance-critical function, there exists:
1. A **C reference implementation** (suffix `_c`)
2. Zero or more **SIMD implementations** (suffixes like `_sse4_1`, `_avx2`, `_avx512`, `_neon`, `_sve`)
3. A **global function pointer** that is set at initialization to the best available implementation

The two RTCD tables are:
- **`aom_dsp_rtcd`** (~2300 lines): Encoder-specific DSP functions (transforms, SAD, motion estimation, rate-distortion)
- **`common_dsp_rtcd`** (~2900 lines): Functions shared between encoder and decoder (convolution, prediction, inverse transforms, blending, loop filtering)

---

## 15.2 How RTCD Works

### 15.2.1 Declaration Pattern

In the header files (`aom_dsp_rtcd.h`, `common_dsp_rtcd.h`), each dispatched function follows this pattern:

```c
// C reference declaration
ReturnType svt_function_name_c(params...);

// Function pointer (extern in most translation units, defined in one)
RTCD_EXTERN ReturnType (*svt_function_name)(params...);
```

The `RTCD_EXTERN` macro resolves to `extern` everywhere except in the single `.c` file that defines `AOM_RTCD_C` (or `RTCD_C`), where it resolves to nothing (defining the pointer).

### 15.2.2 Initialization

At encoder startup, two setup functions are called:

```c
void svt_aom_setup_common_rtcd_internal(EbCpuFlags flags);  // common_dsp_rtcd.c
void svt_aom_setup_rtcd_internal(EbCpuFlags flags);         // aom_dsp_rtcd.c
```

Each function:
1. Acquires a lazily-initialized mutex (thread-safe, uses `DEFINE_ONCE_MUTEX`)
2. Checks a static `first_call_setup` flag (avoids redundant initialization)
3. For each function pointer:
   a. Sets it to the C reference implementation
   b. Conditionally upgrades to SIMD variants based on the `flags` bitmask

### 15.2.3 The SET_FUNCTIONS Macro (x86_64)

```c
#define SET_FUNCTIONS(ptr, c, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512)
    do {
        CHECK_PTR_IS_NOT_SET(ptr);
        SET_FUNCTION_C(ptr, c);
        SET_FUNCTIONS_X86(ptr, mmx, sse, sse2, sse3, ssse3, sse4_1, sse4_2, avx, avx2, avx512);
        CHECK_PTR_IS_SET(ptr);
    } while (0)
```

Each `SET_FUNCTION(ptr, func, flag)` checks:
- `func` is not NULL
- The corresponding CPU flag is set in `flags`

If both conditions are true, the pointer is updated. Since they are applied in order from oldest to newest ISA extension, the last match wins (i.e., the most advanced available implementation).

### 15.2.4 The SET_FUNCTIONS Macro (AArch64)

```c
#define SET_FUNCTIONS_AARCH64(ptr, neon, neon_dotprod, sve, neoverse_v2)
    SET_FUNCTION(ptr, neon, EB_CPU_FLAGS_NEON)
    SET_FUNCTION_NEON_DOTPROD(ptr, neon_dotprod)
    SET_FUNCTION_SVE(ptr, sve)
    SET_FUNCTION_NEOVERSE_V2(ptr, neoverse_v2)
```

AArch64 dispatch supports: NEON (baseline), NEON dot product, SVE, and Neoverse V2 specific optimizations.

---

## 15.3 CPU Feature Detection

### 15.3.1 API

```c
EbCpuFlags svt_aom_get_cpu_flags();        // Detect all available features
EbCpuFlags svt_aom_get_cpu_flags_to_use(); // Possibly restricted by env var
```

### 15.3.2 x86_64 Feature Flags

| Flag | Value | ISA |
|------|-------|-----|
| `EB_CPU_FLAGS_MMX` | bit 0 | MMX |
| `EB_CPU_FLAGS_SSE` | bit 1 | SSE |
| `EB_CPU_FLAGS_SSE2` | bit 2 | SSE2 |
| `EB_CPU_FLAGS_SSE3` | bit 3 | SSE3 |
| `EB_CPU_FLAGS_SSSE3` | bit 4 | SSSE3 |
| `EB_CPU_FLAGS_SSE4_1` | bit 5 | SSE4.1 |
| `EB_CPU_FLAGS_SSE4_2` | bit 6 | SSE4.2 |
| `EB_CPU_FLAGS_AVX` | bit 7 | AVX |
| `EB_CPU_FLAGS_AVX2` | bit 8 | AVX2 |
| `EB_CPU_FLAGS_AVX512F` | bit 9 | AVX-512F |

Detection uses `cpuid` intrinsics on x86_64.

### 15.3.3 AArch64 Feature Flags

| Flag | ISA |
|------|-----|
| `EB_CPU_FLAGS_NEON` | NEON (baseline) |
| `EB_CPU_FLAGS_NEON_DOTPROD` | NEON dot product |
| `EB_CPU_FLAGS_SVE` | SVE |
| `EB_CPU_FLAGS_NEOVERSE_V2` | Neoverse V2 specific |

---

## 15.4 Function Categories in aom_dsp_rtcd

The `aom_dsp_rtcd.h` header (~2300 lines) declares function pointers for encoder-specific DSP operations. Below is a categorized inventory.

### 15.4.1 SSE / Distortion Metrics

| Function | Description |
|----------|-------------|
| `svt_aom_sse` | Sum of Squared Errors (8-bit) |
| `svt_aom_highbd_sse` | SSE (high bit depth) |

### 15.4.2 Block Hashing

| Function | Description |
|----------|-------------|
| `svt_av1_get_crc32c_value` | CRC-32C hash computation |

### 15.4.3 Compound Prediction

| Function | Description |
|----------|-------------|
| `svt_av1_wedge_compute_delta_squares` | Delta squares for wedge mask |
| `svt_av1_wedge_sign_from_residuals` | Wedge sign determination |

### 15.4.4 CDEF (Constrained Directional Enhancement Filter)

| Function | Description |
|----------|-------------|
| `svt_compute_cdef_dist_16bit` | CDEF distortion (16-bit) |
| `svt_compute_cdef_dist_8bit` | CDEF distortion (8-bit) |

### 15.4.5 Restoration Filter Statistics

| Function | Description |
|----------|-------------|
| `svt_av1_compute_stats` | Wiener filter statistics (8-bit) |
| `svt_av1_compute_stats_highbd` | Wiener filter statistics (high bit depth) |
| `svt_av1_lowbd_pixel_proj_error` | Self-guided filter projection error (8-bit) |
| `svt_av1_highbd_pixel_proj_error` | Self-guided filter projection error (high bit depth) |

### 15.4.6 CFL (Chroma from Luma)

| Function | Description |
|----------|-------------|
| `svt_subtract_average` | Subtract DC from prediction buffer |

### 15.4.7 Forward Transforms (Full, N2, N4 variants)

All sizes from 4x4 to 64x64, including rectangular transforms. Three coefficient density variants:
- **Full**: All coefficients computed
- **N2**: Only top-left half of coefficients (used for reduced-precision RDO)
- **N4**: Only top-left quarter of coefficients

Sizes: 4x4, 4x8, 8x4, 4x16, 16x4, 8x8, 8x16, 16x8, 8x32, 32x8, 16x16, 16x32, 32x16, 16x64, 64x16, 32x32, 32x64, 64x32, 64x64

Plus `svt_av1_fwht4x4` (Walsh-Hadamard 4x4).

### 15.4.8 MSE (Mean Squared Error)

| Function | Description |
|----------|-------------|
| `svt_aom_mse16x16` | 16x16 MSE (8-bit) |
| `svt_aom_highbd_mse16x16` | 16x16 MSE (high bit depth) |

### 15.4.9 SAD (Sum of Absolute Differences)

Extensive set of SAD functions for all block sizes, with multiple search patterns:

- **Single-reference SAD**: `svt_aom_sad{W}x{H}` for W x H from 4x4 to 128x128
- **SAD with averaging**: `svt_aom_sad{W}x{H}_avg`
- **Multi-position SAD** (x3, x4, x8): `svt_aom_sad{W}x{H}x{N}d` -- computes SAD at N reference positions simultaneously for motion estimation
- **High bit depth variants**: `svt_aom_highbd_sad{W}x{H}` and `svt_aom_highbd_sad_avg{W}x{H}`

### 15.4.10 Variance

| Function Pattern | Description |
|-----------------|-------------|
| `svt_aom_variance{W}x{H}` | Variance (8-bit) |
| `svt_aom_sub_pixel_variance{W}x{H}` | Sub-pixel variance |
| `svt_aom_sub_pixel_avg_variance{W}x{H}` | Sub-pixel avg variance |
| `svt_aom_highbd_10_variance{W}x{H}` | 10-bit variance |
| `svt_aom_highbd_10_sub_pixel_variance{W}x{H}` | 10-bit sub-pixel variance |

### 15.4.11 Motion Estimation

| Function | Description |
|----------|-------------|
| `svt_ext_sad_calculation_8x8_16x16` | 8x8 and 16x16 SAD for ME |
| `svt_ext_sad_calculation_32x32_64x64` | 32x32 and 64x64 SAD for ME |
| `svt_ext_all_sad_calculation_8x8_16x16` | All 8x8/16x16 SAD positions |
| `svt_initialize_buffer_32bits` | Zero initialization |
| `svt_sad_loop_kernel` | Core ME search loop |
| `svt_pme_sad_loop_kernel` | Predictive ME search loop |
| `svt_nxm_sad_kernel_sub_sampled` | Subsampled SAD kernel |

### 15.4.12 Temporal Filtering

| Function | Description |
|----------|-------------|
| `svt_aom_apply_filtering` | TF filter application (8-bit) |
| `svt_aom_apply_filtering_highbd` | TF filter application (high bit depth) |
| `svt_aom_apply_filtering_central` | TF central frame handling |
| `svt_aom_apply_filtering_central_highbd` | TF central (high bit depth) |

### 15.4.13 Picture Operations

| Function | Description |
|----------|-------------|
| `svt_aom_picture_average_kernel` | Average two pictures |
| `svt_picture_histogram` | Compute histogram |
| `svt_compute8x8_satd` | 8x8 SATD (Hadamard) |
| `svt_aom_compute_mean_8x8` | 8x8 mean |
| `svt_aom_compute_mean_of_squared_values8x8` | 8x8 mean of squares |

### 15.4.14 Quantization / Entropy Coding

| Function | Description |
|----------|-------------|
| `svt_aom_quantize_b` | Forward quantization |
| `svt_aom_quantize_b_flat` | Flat quantization |
| `svt_aom_highbd_quantize_b_flat` | High bit depth flat quantization |
| `svt_aom_txb_init_levels` | Initialize coefficient levels |
| `svt_av1_txb_init_levels_signs` | Initialize levels and signs |

### 15.4.15 Film Grain

| Function | Description |
|----------|-------------|
| `svt_av1_add_film_grain_run` | Add film grain synthesis |
| `svt_av1_build_compound_diffwtd_mask_d16` | Diffwtd compound mask |

### 15.4.16 Noise Estimation

| Function | Description |
|----------|-------------|
| `svt_aom_noise_tx_filter` | Noise estimation via transforms |
| `svt_aom_flat_block_finder_extract_block` | Flat block detection |
| `svt_aom_noise_model_update` | Noise model update |

---

## 15.5 Function Categories in common_dsp_rtcd

The `common_dsp_rtcd.h` header (~2900 lines) declares function pointers for functions shared between encoder and decoder.

### 15.5.1 Memory Operations

| Function | Description |
|----------|-------------|
| `svt_memcpy` | Optimized memcpy (may use SIMD) |
| `svt_av1_copy_wxh_8bit` | Copy WxH block (8-bit) |
| `svt_av1_copy_wxh_16bit` | Copy WxH block (16-bit) |

### 15.5.2 Blending

| Function | Description |
|----------|-------------|
| `svt_aom_blend_a64_vmask` | Vertical mask blend (8-bit) |
| `svt_aom_blend_a64_hmask` | Horizontal mask blend (8-bit) |
| `svt_aom_blend_a64_mask` | 2D mask blend (8-bit) |
| `svt_aom_highbd_blend_a64_mask` | 2D mask blend (high bit depth) |
| `svt_aom_highbd_blend_a64_vmask_16bit` | Vertical mask blend (16-bit) |
| `svt_aom_highbd_blend_a64_hmask_16bit` | Horizontal mask blend (16-bit) |

### 15.5.3 CFL (Chroma from Luma)

| Function | Description |
|----------|-------------|
| `svt_cfl_predict_lbd` | CFL prediction (8-bit) |
| `svt_cfl_predict_hbd` | CFL prediction (high bit depth) |
| `svt_cfl_luma_subsampling_420_lbd` | CFL luma subsampling (8-bit) |
| `svt_cfl_luma_subsampling_420_hbd` | CFL luma subsampling (high bit depth) |

### 15.5.4 Intra Prediction

| Function | Description |
|----------|-------------|
| `svt_av1_filter_intra_predictor` | Filter intra prediction |
| `svt_av1_filter_intra_edge` | Edge filtering (8-bit) |
| `svt_av1_filter_intra_edge_high` | Edge filtering (high bit depth) |
| `svt_av1_upsample_intra_edge` | Edge upsampling |
| `svt_av1_highbd_dr_prediction_z2` | Directional prediction zone 2 (high bit depth) |

### 15.5.5 Inverse Transforms

All square sizes: 4x4, 8x8, 16x16, 32x32, 64x64
All rectangular sizes: 4x8, 8x4, 4x16, 16x4, 8x16, 16x8, 8x32, 32x8, 16x32, 32x16, 16x64, 64x16, 32x64, 64x32

| Function Pattern | Description |
|-----------------|-------------|
| `svt_av1_inv_txfm2d_add_{W}x{H}` | Inverse transform + add (16-bit output) |
| `svt_av1_inv_txfm_add` | Generic inverse transform dispatcher |

### 15.5.6 Pixel Packing / Unpacking

| Function | Description |
|----------|-------------|
| `svt_compressed_packmsb` | Pack 8+nbit to 16-bit |
| `svt_convert_8bit_to_16bit` | 8-bit to 16-bit conversion |
| `svt_convert_16bit_to_8bit` | 16-bit to 8-bit conversion |
| `svt_pack2d_16_bit_src_mul4` | Pack 2D 16-bit source |
| `svt_aom_un_pack2d_16_bit_src_mul4` | Unpack 2D 16-bit source |

### 15.5.7 Residual Computation

| Function | Description |
|----------|-------------|
| `svt_residual_kernel8bit` | 8-bit residual (input - pred) |
| `svt_residual_kernel16bit` | 16-bit residual |

### 15.5.8 Distortion Metrics

| Function | Description |
|----------|-------------|
| `compute8x8_satd_u8` | 8x8 SATD with DC value |
| `sum_residual8bit` | Sum of residual values |
| `svt_full_distortion_kernel_cbf_zero32_bits` | Full distortion (zero CBF) |
| `svt_full_distortion_kernel32_bits` | Full distortion (32-bit coeffs) |
| `svt_spatial_full_distortion_kernel` | Spatial full distortion (8-bit) |
| `svt_full_distortion_kernel16_bits` | Full distortion (16-bit) |

### 15.5.9 Inter Prediction / Convolution

Extensive convolution functions for motion compensation:

| Function Pattern | Description |
|-----------------|-------------|
| `svt_av1_convolve_2d_copy_sr` | 2D copy (no filtering) |
| `svt_av1_convolve_2d_sr` | 2D single-reference convolution |
| `svt_av1_convolve_x_sr` | Horizontal-only convolution |
| `svt_av1_convolve_y_sr` | Vertical-only convolution |
| `svt_av1_convolve_2d_scale` | Scaled 2D convolution |
| `svt_av1_jnt_convolve_*` | Joint (compound) convolution variants |
| `svt_av1_highbd_convolve_*` | High bit depth convolution variants |
| `svt_av1_highbd_jnt_convolve_*` | High bit depth compound convolution |

### 15.5.10 Wiener / Self-Guided Restoration

| Function | Description |
|----------|-------------|
| `svt_av1_wiener_convolve_add_src` | Wiener filter convolution (8-bit) |
| `svt_av1_highbd_wiener_convolve_add_src` | Wiener filter convolution (high bit depth) |
| `svt_apply_selfguided_restoration` | Self-guided filter application |
| `svt_av1_selfguided_restoration` | Self-guided restoration |

### 15.5.11 Deblocking / Loop Filter

| Function | Description |
|----------|-------------|
| `svt_aom_lpf_horizontal_*` | Horizontal loop filter (4, 6, 8, 14 taps) |
| `svt_aom_lpf_vertical_*` | Vertical loop filter (4, 6, 8, 14 taps) |
| `svt_aom_highbd_lpf_horizontal_*` | High bit depth horizontal LF |
| `svt_aom_highbd_lpf_vertical_*` | High bit depth vertical LF |

### 15.5.12 CDEF Application

| Function | Description |
|----------|-------------|
| `svt_cdef_filter_block` | CDEF filter block |
| `svt_cdef_copy_rect8_8bit_to_16bit` | Copy for CDEF (8->16 bit) |
| `svt_cdef_copy_rect8_16bit_to_16bit` | Copy for CDEF (16->16 bit) |

### 15.5.13 Film Grain (Decode-Side)

| Function | Description |
|----------|-------------|
| `svt_aom_fgs_32x32_8bit` | Film grain synthesis 32x32 (8-bit) |
| `svt_aom_fgs_32x32_16bit` | Film grain synthesis 32x32 (16-bit) |

### 15.5.14 AVC-Style Motion Compensation

| Function | Description |
|----------|-------------|
| `avc_style_luma_interpolation_filter` | AVC-style luma interpolation for HME |

### 15.5.15 Palette / K-Means

| Function | Description |
|----------|-------------|
| `svt_av1_k_means_dim1` | 1D k-means for palette |
| `svt_av1_k_means_dim2` | 2D k-means for palette |
| `svt_av1_calc_indices_dim1` | 1D palette index calculation |
| `svt_av1_calc_indices_dim2` | 2D palette index calculation |

---

## 15.6 C Reference Implementations (`Source/Lib/C_DEFAULT/`)

The `C_DEFAULT` directory contains portable C implementations that serve as the baseline for RTCD dispatch:

| File | Contents |
|------|----------|
| `blend_a64_mask_c.c` | Alpha blending with 64-level masks |
| `cfl_c.c` | Chroma-from-Luma prediction |
| `compute_sad_c.c` | SAD computation for all block sizes |
| `encode_txb_ref_c.c` | Transform block encoding reference |
| `filterintra_c.c` | Filter intra prediction modes |
| `inter_prediction_c.c` | Inter prediction convolution |
| `intra_prediction_c.c` | Intra prediction modes |
| `pack_unpack_c.c` | Pixel bit-depth packing/unpacking |
| `picture_operators_c.c` | Picture-level operations (copy, diff, SAD) |
| `sad_av1.c` | AV1-specific SAD functions |
| `variance.c` | Variance computation for all block sizes |

These files are always compiled and linked. The RTCD system starts with these as defaults, then upgrades to SIMD implementations where available.

---

## 15.7 Global State (`Source/Lib/Globals/`)

The `Globals` directory contains:

| File | Purpose |
|------|---------|
| `enc_handle.c` | Main encoder API entry points: `svt_av1_enc_init`, `svt_av1_enc_deinit`, etc. This is where RTCD initialization is triggered via `svt_aom_setup_rtcd_internal` and `svt_aom_setup_common_rtcd_internal`. |
| `enc_handle.h` | Encoder handle structure declarations |
| `enc_settings.c/h` | Encoder configuration validation and defaults |
| `metadata_handle.c/h` | Metadata (HDR, timecode) handling |

The encoder handle initialization calls both RTCD setup functions with the detected CPU flags, which populates all function pointers for the lifetime of the encoder instance.

---

## 15.8 Porting Notes

### 15.8.1 What to Port

For an algorithm-accurate port without SIMD:
- Implement every `_c` function. These are the normative reference implementations.
- The RTCD dispatch mechanism itself can be replaced with direct function calls (no function pointers needed if not doing runtime dispatch).

### 15.8.2 SIMD Strategy

If adding SIMD to a port:
- The function-pointer pattern maps cleanly to trait objects, vtables, or function pointer tables in most languages.
- CPU feature detection must be reimplemented for the target platform.
- The hierarchical flag system (each ISA level implies all previous ones) simplifies dispatch.

### 15.8.3 Function Count

Approximate function pointer counts:
- `aom_dsp_rtcd.h`: ~600 dispatched functions
- `common_dsp_rtcd.h`: ~500 dispatched functions
- Total: ~1100 dispatched functions

Many of these are size-specialized variants (e.g., SAD for every block size). The distinct algorithm count is much smaller (~50-80 unique algorithms).
