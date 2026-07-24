# Testing Infrastructure

## Overview

SVT-AV1 has three separate test binaries, each built and run independently:

| Binary | Directory | Purpose |
|--------|-----------|---------|
| `SvtAv1UnitTests` | `test/` | SIMD-vs-C correctness for DSP kernels |
| `SvtAv1E2ETests` | `test/e2e_test/` | Encode-decode round-trip conformance |
| `SvtAv1ApiTests` | `test/api_test/` | Public API contract validation |

All three use Google Test (vendored in `third_party/googletest/`). The build system is CMake, with `test/CMakeLists.txt` as the root that includes `api_test/` and `e2e_test/` as subdirectories.

A fourth component, `test/benchmarking/`, is a Python-based codec comparison framework (not Google Test) for BD-rate analysis against other codecs.

### Build-Time File Classification

The unit test CMake classifies source files into three groups that determine platform portability:

- **`arch_neutral_files`**: Tests that run on any platform (no SIMD). Includes: `BitstreamWriterTest.cc`, `FilmGrainTest.cc`, `FwdTxfm2dApproxTest.cc`, `GlobalMotionUtilTest.cc`, `IntraBcUtilTest.cc`, `ResizeTest.cc`, `ssim_test.cc`, and all shared infrastructure.
- **`multi_arch_files`**: Tests that have both x86 and ARM instantiations. This is the bulk of the SIMD tests (SAD, variance, convolution, CDEF, etc.).
- **`x86_arch_files`**: Tests only compiled for x86_64. Includes: `noise_model_test.cc`, `highbd_intra_prediction_tests.cc`, `FFTTest.cc`, `ForwardtransformTests.cc`, `FwdTxfm1dTest.cc`, `InvTxfm1dTest.cc`, `FwdTxfm2dTest.cc`, `MotionEstimationTest.cc`, `PsnrTest.cc`, `frame_error_test.cc`.

The unit test binary links against object libraries for each ISA tier: `C_DEFAULT`, `ASM_SSE2`, `ASM_SSSE3`, `ASM_SSE4_1`, `ASM_AVX2`, `ASM_AVX512` (x86), and `ASM_NEON`, `ASM_NEON_DOTPROD`, `ASM_NEON_I8MM`, `ASM_SVE`, `ASM_SVE2` (ARM).

### Runtime CPU Feature Detection

The main entry point (`test/svt_av1_test.cc`) queries `svt_aom_get_cpu_flags_to_use()` at startup and appends negative gtest filters for unsupported ISA extensions. For example, if AVX512 is not available, all tests with `AVX512` in their name are excluded. This filtering uses naming conventions: test suite prefixes like `AVX2`, `SSE4_1`, `NEON`, `SVE` etc. are matched via the patterns `*ISA.*`, `*ISA/*`, and `*ISA_*`.

### Environment Setup

`test/TestEnv.c` provides `setup_test_env()` which initializes the RTCD function pointer tables (both `common_rtcd` and `rtcd`) based on detected CPU capabilities. Unit test fixtures call this in their constructor or `SetUp()` to ensure SIMD dispatch is active. A companion `reset_test_env()` resets dispatch to C-only (used to get the reference implementation for comparison).

## Test Architecture

### Framework & Utilities

#### Random Number Generators

Two random generators are available:

1. **`svt_av1_test_tool::SVTRandom`** (`test/random.h`): C++11 `mt19937` wrapper with deterministic seed (13596). Supports integer ranges, bit-width-based bounds, float ranges, and explicit seed reset.

2. **`libaom_test::ACMRandom`** (`test/acm_random.h`): Ported from AOM. Uses Google Test's internal `Random` class with deterministic seed 0xbaba. Provides `Rand8()`, `Rand16()`, `Rand12()`, `Rand8Extremes()`, `PseudoUniform()`.

Both generators use deterministic seeds, making all tests reproducible.

#### Buffer Utilities

`test/unit_test_utility.c` and `.h` provide:
- `svt_buf_random_void/u8/s16/u16/s32/u32/s64`: Fill buffers with random data
- `svt_buf_random_u16_with_bd`: Random values bounded by bit depth
- `svt_buf_random_u8_to_0_or_255`: Extreme values only
- `svt_buf_compare_u16/s16/u32/s32`: Element-wise buffer comparison with mismatch reporting
- `svt_create_random_aligned_stride`: Generate random stride with alignment constraints

#### Common Macros

`test/unit_test.h` defines:
- `EB_UNIT_TEST_NUM = 10` (default iteration count)
- `TEST_ALLIGN_MALLOC/TEST_ALLIGN_FREE`: Cross-platform aligned allocation (Windows `_aligned_malloc` / POSIX `posix_memalign`)
- `Eb_UNIT_TEST_BUF_SIZE = 0x04000000` (64 MB)

`test/util.h` defines:
- `PI` constant
- `TEST_GET_PARAM(k)` macro for parameterized tests
- `ALIGNED_ADDR` macro for aligning buffer pointers
- `svt_av1_test_tool::round_shift()` for fixed-point rounding

### SIMD Testing Pattern

The dominant pattern across all unit tests is **C-reference-vs-optimized comparison**:

1. **Setup**: Call `setup_test_env()` to populate RTCD tables with optimized functions.
2. **Get function pointers**: The test fixture receives both a C reference function and an optimized (SIMD) function, typically via parameterized test values.
3. **Generate input**: Fill input buffers with random data using `SVTRandom` or `svt_buf_random_*`.
4. **Execute both**: Run C reference and optimized function on identical inputs.
5. **Compare outputs**: Assert output buffers are identical (for exact-match tests) or within tolerance (for approximation tests).
6. **Speed tests**: Many fixtures include a `DISABLED_Speed` or `DISABLED_SpeedTest` variant that runs both functions in a tight loop and reports timing. These are disabled by default and run manually for profiling.

Parameterization follows a consistent pattern:
```
INSTANTIATE_TEST_SUITE_P(ISA_NAME, TestFixture, ::testing::ValuesIn(test_params));
```
where `ISA_NAME` is `AVX2`, `SSE4_1`, `NEON`, etc. This enables the runtime CPU feature filtering described above.

### E2E Testing Pattern

End-to-end tests use the `SvtAv1E2ETestFramework` class, which orchestrates a full encode-decode cycle:

1. **Configure**: Set encoder parameters via `EncTestSetting` (name-value string pairs).
2. **Initialize**: Create encoder handle, allocate input/output buffers, optionally create reference decoder (libaom) and reconstruction queue.
3. **Encode loop**: Feed YUV frames to encoder, collect compressed output.
4. **Decode**: Feed compressed data to AOM reference decoder.
5. **Compare**: Compare reconstructed frames (from encoder) with decoded frames (from reference decoder). Pixel-exact match is required.
6. **PSNR**: Compute PSNR between source and decoded output.

The E2E framework runs tests as **death tests** (`EXPECT_EXIT`), isolating each encode session in a child process so crashes do not kill the test runner.

**Video sources** are abstracted behind a `VideoSource` base class with three implementations:
- `YuvVideoSource`: Raw YUV files
- `Y4MVideoSource`: Y4M container files
- `DummyVideoSource`: Procedurally generated color bar patterns (no file dependency)

Test vectors are downloaded via CMake (`make TestVectors`) from a URL, with SHA1 checksums verified. The path is set via `SVT_AV1_TEST_VECTOR_PATH` environment variable.

### API Testing Pattern

API tests directly call the public encoder API (`svt_av1_enc_init_handle`, `svt_av1_enc_set_parameter`, etc.) and validate:
- Null pointer handling (expects `EB_ErrorBadParameter`, not crashes)
- Parameter validation (default/valid/invalid values per parameter)
- Resource lifecycle (init/deinit cycles)
- Multi-encoder thread safety

The parameter validation uses a macro-driven framework where each parameter has defined `default`, `valid`, and `invalid` vectors in `test/api_test/params.h`, and a `DEFINE_PARAM_TEST_CLASS` / `PARAM_TEST` macro pair auto-generates a test fixture that checks all three categories.

## Test Inventory

### Transform Tests

#### `FwdTxfm1dTest.cc` (x86 only)

- **Fixture**: `AV1FwdTxfm1dTest` (parameterized)
- **Tests**: `run_fwd_accuracy_check`
- **Validates**: Forward 1D transforms (DCT, ADST, identity) match double-precision reference within tolerance
- **Pattern**: Random input -> C integer implementation -> compare with `reference_txfm_1d()` (double-precision math in `TxfmRef.cc`)
- **Functions tested**: `svt_av1_fdct{4,8,16,32,64}_new`, `svt_av1_fadst{4,8,16}_new`, `svt_av1_fidentity{4,8,16,32}_c`
- **Source**: `Source/Lib/C_DEFAULT/` transform functions

#### `InvTxfm1dTest.cc` (x86 only)

- **Fixture**: `AV1InvTxfm1dTest` (parameterized)
- **Tests**: `run_inv_accuracy_check`
- **Validates**: Inverse 1D transforms match double-precision reference
- **Pattern**: Same as FwdTxfm1dTest but for inverse transforms
- **Functions tested**: `svt_av1_idct{4,8,16,32,64}_new`, `svt_av1_iadst{4,8,16}_new`, `svt_av1_iidentity{4,8,16,32}_c`

#### `FwdTxfm2dTest.cc` (x86 only)

- **Fixture**: `AV1FwdTxfm2dTest` (parameterized over all TX_SIZES_ALL)
- **Tests**: `run_fwd_accuracy_check`
- **Validates**: Complete 2D forward transform (all tx_type x tx_size combinations) matches double-precision reference `reference_txfm_2d()`
- **Pattern**: Random 16-bit input -> C 2D transform -> compare with double-precision reference, checking max error per coefficient
- **Functions tested**: `svt_av1_transform_two_d_*_c`, `svt_av1_fwd_txfm2d_*_c` (all 19 size variants)

#### `FwdTxfm2dAsmTest.cc`

- **Fixture**: `FwdTxfm2dAsmTest` (parameterized)
- **Tests**: `match_test`, `DISABLED_speed_test`
- **Validates**: SIMD forward 2D transforms match C reference for full, N2 (half), and N4 (quarter) coefficient modes
- **Pattern**: C-vs-SIMD comparison across all tx_type x tx_size
- **ISA variants**: SSE4_1, AVX2, AVX512, NEON, SVE (extensive instantiations for each)
- **Functions tested**: RTCD dispatched `svt_av1_fwd_txfm2d_*` for all sizes

#### `FwdTxfm2dApproxTest.cc` (arch-neutral)

- **Fixture**: `FwdTxfm2dApproxTest` (parameterized)
- **Tests**: `run_fwd_accuracy_check`
- **Validates**: Forward 2D transform C implementation produces coefficients within acceptable error bounds of double-precision reference
- **Pattern**: Random input -> C transform -> compare with `reference_txfm_2d()` -> check error <= libaom-defined threshold

#### `InvTxfm2dAsmTest.cc`

- **Fixtures**: `InvTxfm2dAsmSqrTest`, `InvTxfm2dAsmType1Test`, `InvTxfm2dAsmType2Test`, `InvTxfm2dAddTest`, `HandleTransformTest`
- **Tests**: `sqr_txfm_match_test`, `rect_type1_txfm_match_test`, `rect_type2_txfm_match_test`, `svt_av1_inv_txfm_add`, `match_test`, `DISABLED_speed_test`
- **Validates**: SIMD inverse 2D transforms match C reference for square, rectangular type1, rectangular type2, and inverse-transform-add operations
- **Pattern**: Forward-transform input -> inverse transform with C and SIMD -> compare outputs
- **ISA variants**: SSE4_1, AVX2, AVX512, NEON

#### `ForwardtransformTests.cc` (x86 only, AVX512)

- **Fixture**: None (standalone TEST)
- **Tests**: `AVX512_ForwardTransformTest/av1_frwd_txfm_kernels`
- **Validates**: AVX512 forward transform kernels match C reference
- **Pattern**: Iterates all tx_size x tx_type, compares AVX512 output with C

#### `EncodeTxbAsmTest.cc`

- **Fixtures**: `EncodeTxbTest`, `EncodeTxbInitLevelTest`
- **Tests**: `GetNzMapTest`, `txb_init_levels_match`, `DISABLED_txb_init_levels_speed`
- **Validates**: Entropy coding coefficient context (NZ map) and txb init levels SIMD match C
- **ISA variants**: AVX2, AVX512, NEON, SVE

### Prediction Tests

#### `intrapred_test.cc`

- **Fixtures**: `HighbdIntraPredTest`, `LowbdIntraPredTest`
- **Tests**: `match_test`
- **Validates**: All standard intra prediction modes (DC, H, V, smooth, smooth_h, smooth_v, paeth) for both LBD and HBD
- **Pattern**: C-vs-SIMD for each predictor function, all block sizes
- **ISA variants**: SSE2, AVX2, SSSE3, NEON

#### `intrapred_dr_test.cc`

- **Fixtures**: `LowbdZ1PredTest`, `LowbdZ2PredTest`, `LowbdZ3PredTest`, `HighbdZ1PredTest`, `HighbdZ2PredTest`, `HighbdZ3PredTest`
- **Tests**: `MatchTest` (for each)
- **Validates**: Directional intra prediction (zones 1, 2, 3) for LBD and HBD across all block sizes and angles
- **ISA variants**: AVX2, SSE4_1, NEON

#### `intrapred_edge_filter_test.cc`

- **Fixtures**: `UpsampleTest`, `LowbdFilterEdgeTest`, `HighbdFilterEdgeTest`
- **Tests**: `RunTest`
- **Validates**: Edge filtering and upsampling for directional intra prediction
- **ISA variants**: SSE4_1, NEON

#### `intrapred_cfl_test.cc`

- **Fixtures**: `LbdCflPredTest`, `HbdCflPredTest`, `AomUpsampledPredTest`, `CflLumaSubsamplingLbdTest`, `CflLumaSubsamplingHbdTest`
- **Tests**: `MatchTest`
- **Validates**: CfL (Chroma from Luma) prediction, upsampled prediction, and luma subsampling
- **ISA variants**: AVX2, NEON, SSE4_1, SSSE3

#### `FilterIntraPredTest.cc`

- **Fixture**: `FilterIntraPredTest`
- **Tests**: `RunCheckOutput`
- **Validates**: Filter intra prediction mode SIMD matches C
- **ISA variants**: SSE4_1, AVX2, NEON

#### `highbd_intra_prediction_tests.cc` (x86 only, AVX512)

- **Fixture**: None (standalone TESTs)
- **Tests**: `aom_dc_top_predictor_kernels`, `aom_dc_left_predictor_kernels`, `aom_dc_predictor_kernels`, `aom_h_predictor_kernels`, `aom_v_predictor_kernels`, `aom_highbd_smooth_predictor_kernels`, `aom_highbd_smooth_v_predictor_kernels`, `aom_highbd_smooth_h_predictor_kernels`
- **Validates**: AVX512 high-bit-depth intra predictor implementations
- **Pattern**: C-vs-AVX512 for all block sizes per predictor type

### Motion Estimation Tests

#### `MotionEstimationTest.cc` (x86 only)

- **Fixture**: None (standalone TESTs)
- **Tests**: `sadMxN_match`, `sadMxNx4d_match`, `DISABLED_sadMxN_speed`, `DISABLED_sadMxNx4d_speed` (for both AVX2 and AVX512)
- **Validates**: Motion estimation SAD functions (single and 4-direction) match C reference
- **Pattern**: Random blocks -> compute SAD with C and SIMD -> compare

#### `SadTest.cc`

- **Fixtures**: `SADTest`, `sad_LoopTest`, `Allsad8x8_CalculationTest`, `Allsad32x32_CalculationTest`, `Extsad8x8_CalculationTest`, `Extsad32x32_CalculationTest`, `InitializeBuffer32`, `SADTestSubSample16bit`, `PmeSadLoopTest`
- **Tests**: `SADTest`, `sad_LoopTest`, `DISABLED_sad_LoopSpeedTest`, `check_sad8x8`, `check_sad32x32`, `InitializeBuffer`, `SADTestSubSample16bit`, `DISABLED_Speed`, `PmeSadLoopTest`, `DISABLED_PmeSadLoopSpeedTest`
- **Validates**: SAD computation for all block sizes, loop-based SAD, 8x8 aggregation, 32x32 aggregation, extended SAD, 16-bit subsampled SAD, PME SAD loop
- **ISA variants**: AVX2, AVX512, NEON, NEON_DOTPROD, SSE2

#### `SatdTest.cc`

- **Fixture**: `SatdTest`
- **Tests**: `MinValue`, `MaxValue`, `Match`, `DISABLED_Speed`
- **Validates**: Sum of Absolute Transformed Differences for various block sizes
- **Pattern**: Extreme values + random comparison between C and SIMD
- **ISA variants**: AVX2, NEON

#### `corner_match_test.cc`

- **Fixture**: `AV1CornerMatchTest`
- **Tests**: `CheckOutput`, `DISABLED_Speed`
- **Validates**: Corner matching for global motion estimation
- **ISA variants**: SSE4_1, AVX2, NEON, NEON_DOTPROD, NEON_I8MM

#### `GlobalMotionUtilTest.cc` (arch-neutral)

- **Fixture**: `RansacIntTest`
- **Tests**: `CheckOutput`
- **Validates**: RANSAC algorithm for global motion parameter estimation
- **Pattern**: Synthetic correspondence points -> verify estimated model matches expected output

### Convolution / Interpolation Tests

#### `convolve_test.cc`

- **Fixtures**: `AV1LbdJntConvolveTest`, `AV1LbdSrConvolveTest`, `AV1HbdJntConvolveTest`, `AV1HbdSrConvolveTest`
- **Tests**: `MatchTest`, `DISABLED_SpeedTest`
- **Validates**: 2D convolution for both joint compound (Jnt) and single-reference (Sr) modes, LBD and HBD. Covers all four sub-types: 2D, X-only, Y-only, and Copy.
- **ISA variants**: SSE2, SSSE3, SSE4_1, AVX2, AVX512, NEON, NEON_DOTPROD, NEON_I8MM, SVE, SVE2

#### `Convolve8Test.cc`

- **Fixture**: `Convolve8Test`
- **Tests**: `GuardBlocks`, `MatchesReferenceSubpixelFilter`, `FilterExtremes`, `DISABLED_Speed`
- **Validates**: 8-tap convolution filters with guard block verification, subpixel filter accuracy, and extreme value handling
- **ISA variants**: SSSE3, AVX2, NEON, NEON_DOTPROD, NEON_I8MM

#### `av1_convolve_scale_test.cc`

- **Fixtures**: `LowBDConvolveScaleTest`, `HighBDConvolveScaleTest`
- **Tests**: `Check`, `DISABLED_Speed`
- **Validates**: Scaled convolution (for super-resolution and reference scaling)
- **ISA variants**: SSE4_1, AVX2, NEON, SSSE3

#### `warp_filter_test.cc` / `warp_filter_test_util.cc`

- **Fixtures**: `AV1WarpFilterTest`, `AV1HighbdWarpFilterTest`
- **Tests**: `CheckOutput`, `DISABLED_Speed`
- **Validates**: Warped motion filter for affine motion compensation, both LBD and HBD
- **ISA variants**: SSE4_1, AVX2, NEON, SVE

#### `wiener_convolve_test.cc`

- **Fixtures**: `AV1WienerConvolveLbdTest`, `AV1WienerConvolveHbdTest`
- **Tests**: `random_test`, `DISABLED_speed_test`
- **Validates**: Wiener filter convolution for loop restoration
- **ISA variants**: SSE2, AVX2, NEON, NEON_I8MM

### Filter Tests

#### `DeblockTest.cc`

- **Fixtures**: `LbdLoopFilterTest`, `HbdLoopFilterTest`
- **Tests**: `MatchTestRandomData`
- **Validates**: Deblocking loop filter for various filter lengths (4, 6, 8, 14) at both LBD and HBD
- **Pattern**: Random edge data -> apply C and SIMD filters -> compare
- **ISA variants**: SSE2, NEON

#### `CdefTest.cc`

- **Fixtures**: `CDEFBlockTest`, `CDEFFindDirTest`, `CDEFFindDirDualTest`, `CDEFCopyRectTest`, `CDEFComputeCdefDist16Bit`, `CDEFComputeCdefDist8BitTest`, `CDEFSearchOneDualTest`
- **Tests**: `MatchTest`, `DISABLED_SpeedTest` (for block), `test_match` (for others), `DISABLED_test_speed`
- **Validates**: CDEF block filtering, direction finding (single and dual), rect copy, distortion computation (8-bit and 16-bit), and search-one-dual optimization
- **ISA variants**: SSE2, SSE4_1, AVX2, AVX512, NEON

#### `selfguided_filter_test.cc`

- **Fixtures**: `AV1SelfguidedFilterTest`, `AV1HighbdSelfguidedFilterTest`
- **Tests**: `CorrectnessTest`, `DISABLED_SpeedTest`
- **Validates**: Self-guided restoration filter for LBD and HBD
- **ISA variants**: SSE4_1, AVX2, NEON

#### `SelfGuidedUtilTest.cc`

- **Fixtures**: `PixelProjErrorLbdTest`, `PixelProjErrorHbdTest`, `GetProjSubspaceTestLbd`, `GetProjSubspaceTestHbd`
- **Tests**: `MatchTestWithRandomValue`, `MatchTestWithRandomSizeAndValue`, `MatchTestWithExtremeValue`, `DISABLED_SpeedTest`, `MatchTest`
- **Validates**: Pixel projection error computation and projection subspace calculation for self-guided filter parameter estimation
- **ISA variants**: SSE4_1, AVX2, NEON, SVE2

#### `RestorationPickTest.cc`

- **Fixtures**: `av1_compute_stats_test`, `av1_compute_stats_test_hbd`
- **Tests**: `match`, `DISABLED_speed`
- **Validates**: Wiener filter statistics computation for LBD and HBD
- **ISA variants**: SSE4_1, AVX2, AVX512, NEON

### Quantization Tests

#### `QuantAsmTest.cc`

- **Fixtures**: `QuantizeBTest`, `QuantizeBQmTest`
- **Tests**: `input_zero_all`, `input_dcac_minmax_q_n`, `input_random_dc_only`, `input_random_all_q_all`
- **Validates**: Quantization (with and without QM matrices) across all QP values, zero input, extreme DC/AC values, random input
- **ISA variants**: AVX2, NEON, SVE

#### `quantize_func_test.cc`

- **Fixtures**: `QuantizeLbdTest`, `QuantizeHbdTest`, `QuantizeQmTest`, `QuantizeQmHbdTest`, `ComputeCulLevelTest`
- **Tests**: `ZeroInput`, `LargeNegativeInput`, `DcOnlyInput`, `RandomInput`, `MultipleQ`, `CoeffHalfDequant`, `DISABLED_Speed`, `test_match`
- **Validates**: AOM-style quantization functions for LBD, HBD, and QM variants; also CUL level computation
- **ISA variants**: SSE4_1, AVX2, NEON, SVE

### Variance & Distortion Tests

#### `VarianceTest.cc`

- **Fixtures**: `MseTest`, `MseTestHighbd`, `SumSquareTest`, `VarianceTest`, `SubpelVarianceTest`
- **Tests**: `MatchTest`, `MaxTest`, `ConstTest`, `ZeroTest`, `OneQuarterTest`, `Ref`, `ExtremeRef`
- **Validates**: MSE, high-bit-depth MSE, sum of squares, variance, and sub-pixel variance for all block sizes
- **ISA variants**: SSE2, AVX2, AVX512, SSSE3, NEON, NEON_DOTPROD

#### `HbdVarianceTest.cc`

- **Fixtures**: `HbdVarianceTest`, `HbdSquareVarianceNoRoundTest`
- **Tests**: `ZeroTest`, `MaximumTest`, `MatchTest`
- **Validates**: High-bit-depth variance and square variance (no rounding) for all block sizes
- **ISA variants**: SSE2, AVX2, NEON, SVE

#### `SpatialFullDistortionTest.cc`

- **Fixtures**: `SpatialFullDistortionKernelFuncTest`, `FullDistortionKernel16BitsFuncTest`, `FullDistortionKernel32Bits`, `FullDistortionKernelCbfZero32Bits`
- **Tests**: `Random`, `ExtremeMin`, `ExtremeMax`, `DISABLED_Speed`, `CheckOutput`
- **Validates**: Spatial full distortion (SSD) kernels for 8-bit, 16-bit, and 32-bit coefficient paths, including CBF-zero variant
- **ISA variants**: SSE2, AVX2, AVX512, NEON, SVE

#### `BlockErrorTest.cc`

- **Fixture**: `BlockErrorTest`
- **Tests**: `OperationCheck`, `ExtremeValues`, `DISABLED_Speed`
- **Validates**: Block error (SSD between original and reconstructed coefficients)
- **ISA variants**: AVX2, NEON, SVE

#### `PsnrTest.cc` (x86 only)

- **Fixture**: `PsnrCalcHbdTest`
- **Tests**: `RunPartialAccuracyCheck`
- **Validates**: HBD PSNR calculation accuracy

#### `frame_error_test.cc` (x86 only)

- Tests frame-level error computation

#### `ssim_test.cc` (arch-neutral)

- **Fixtures**: `SsimLbdTest`, `SsimHbdTest`
- **Tests**: `MatchTestWithExtremeValue`, `MatchTestWithRandomValue`
- **Validates**: SSIM computation for 8-bit and 10-bit

### Residual & Hadamard Tests

#### `ResidualTest.cc`

- **Fixtures**: `ResidualKernel8BitTest`, `ResidualKernel16BitTest`
- **Tests**: `MatchTest`, `DISABLED_SpeedTest`
- **Validates**: Residual computation (source - prediction) for 8-bit and 16-bit paths, all block sizes
- **ISA variants**: SSE2, AVX2, AVX512, NEON

#### `hadamard_test.cc`

- **Fixtures**: `HadamardLowbdTest`, `HadamardHighbdTest`
- **Tests**: `CompareReferenceRandom`, `VaryStride`
- **Validates**: Hadamard transform for rate-distortion optimization
- **ISA variants**: SSE2, AVX2, NEON

### Film Grain & Noise Tests

#### `FilmGrainTest.cc` (arch-neutral)

- **Fixtures**: None (standalone TEST), `AddFilmGrainTest`, `DenoiseModelRunTest`
- **Tests**: `parameters_equality`, `MatchTest`, `OutputFilmGrainCheck`
- **Validates**: Film grain parameter equality checking, film grain synthesis matching expected output (golden test), and denoise model integration
- **Pattern**: Known film grain parameters -> synthesize grain -> compare with expected result arrays in `FilmGrainExpectedResult.h`

#### `noise_model_test.cc` (x86 only)

- **Fixture**: None (standalone TESTs)
- **Tests**: `fg_add_block_observations_internal/AVX2`, `fg_pointwise_multiply/AVX2`, `fg_apply_window_to_plane/AVX2`, `fg_noise_tx_filter/AVX2`, `fg_flat_block_finder_extract_block/lbd_AVX2`, `fg_flat_block_finder_extract_block/hbd_AVX2`
- **Validates**: Noise model internal functions (AVX2 vs C)

### Compound & Advanced Feature Tests

#### `CompoundUtilTest.cc`

- **Fixtures**: `LbdCompBlendTest`, `LbdCompBlendD16Test`, `LbdCompBlendHMaskTest`, `LbdCompBlendVMaskTest`, `HbdCompBlendTest`, `HbdCompBlendD16Test`, `HbdCompBlendHMaskTest`, `HbdCompBlendVMaskTest`, `BuildCompDiffwtdMaskTest`, `BuildCompDiffwtdMaskHighbdTest`, `BuildCompDiffwtdMaskD16Test`, `AomSseTest`, `AomSseHighbdTest`, `AomSubtractBlockTest`, `AomHighbdSubtractBlockTest`
- **Tests**: `BlendA64Mask`, `BlendA64MaskD16`, `MatchTest`
- **Validates**: All compound prediction blending operations (mask, D16, H-mask, V-mask, diffwtd mask), SSE computation, block subtraction - both LBD and HBD
- **ISA variants**: SSE4_1, AVX2, NEON

#### `OBMCVarianceTest.cc`

- **Fixtures**: `OBMCVarianceTest`, `OBMCSubPixelVarianceTest`, `CalcTargetWeightedPredTestAbove`, `CalcTargetWeightedPredTestLeft`
- **Tests**: `RunCheckOutput`
- **Validates**: OBMC (Overlapped Block Motion Compensation) variance, sub-pixel variance, and target weighted prediction
- **ISA variants**: SSE4_1, AVX2, NEON

#### `OBMCSadTest.cc`

- **Fixture**: `OBMCsad_Test`
- **Tests**: `RunCheckOutput`
- **Validates**: OBMC SAD computation
- **ISA variants**: AVX2, NEON

#### `WedgeUtilTest.cc`

- **Fixtures**: `WedgeSignFromResidualsTest`, `WedgeComputeDeltaSquaresTest`, `WedgeSseFromResidualsTest`, `AomSumSquaresTest`
- **Tests**: `RandomTest`, `ExtremeTest`, `ComputeDeltaSquareTest`, `MatchTest`
- **Validates**: Wedge compound prediction utilities (sign from residuals, delta squares, SSE from residuals, sum of squares)
- **ISA variants**: SSE2, AVX2, NEON

#### `IntraBcUtilTest.cc` (arch-neutral)

- **Fixture**: `DvValiationTest`
- **Tests**: `IsDvValidate`
- **Validates**: Intra block copy displacement vector validation

### CfL & Subsampling Tests

#### `subtract_avg_cfl_test.cc`

- **Fixture**: `CflSubAvgTest`
- **Tests**: `subtract_average_asm_test`
- **Validates**: CfL average subtraction
- **ISA variants**: AVX2, NEON

### Utility Tests

#### `PictureOperatorTest.cc`

- **Fixture**: `Downsample2DTest`
- **Tests**: `test`
- **Validates**: 2D downsampling operation
- **ISA variants**: SSE2, AVX2, NEON

#### `ResizeTest.cc` (arch-neutral)

- **Fixtures**: `ResizePlaneLbdTest`, `ResizePlaneHbdTest`
- **Tests**: `MatchTestWithZeroValue`, `MatchTestWithRandomValue`, `MatchTestWithExtremeValue`, `DISABLED_SpeedTestWithRandomValue`
- **Validates**: Plane resizing (super-resolution / reference scaling) for LBD and HBD

#### `PackUnPackTest.cc`

- **Fixtures**: `PackMsbTest`, `Unpack2bCompress`, `Pack2dTest`, `UnPack2d16BitTest`
- **Tests**: `PackMsbTest`, `Unpack2bCompress`, `Pack2dTest`, `RunTest`
- **Validates**: Pack/unpack operations for 10-bit data in compressed and 16-bit formats
- **ISA variants**: SSE2, AVX2, NEON

#### `MemTest.cc`

- **Fixtures**: `MemTestLBD`, `MemTestHBD`
- **Tests**: `Match`, `DISABLED_Speed`
- **Validates**: Memory copy/set operations
- **ISA variants**: SSE2, AVX2, NEON

#### `BitstreamWriterTest.cc` (arch-neutral)

- **Fixture**: `BitstreamWriterTest`
- **Tests**: `write_bits_random`, `write_literal_extreme_int`, `write_symbol_no_update`, `write_symbol_with_update`
- **Validates**: Bitstream writer correctness for random bits, extreme integer literals, and arithmetic symbol coding (with and without CDF update)

#### `compute_mean_test.cc`

- **Fixtures**: `ComputeMeanValueTest`, `ComputeMeanSquaredValueTest`, `ComputeMean8x8Test`, `ComputeMeanFour8x8Test`
- **Tests**: `MatchTest`
- **Validates**: Mean and mean-squared computation for 8x8 blocks
- **ISA variants**: SSE2, AVX2, NEON, SVE

#### `PaletteModeUtilTest.cc`

- **Fixtures**: `ColorCountLbdTest`, `ColorCountHbdTest`, `KMeansTest`, `Av1KMeansIndicesDimTest`, `Av1KMeansDimTest`
- **Tests**: `MatchTest`, `MatchTest8Bit`, `MatchTest10Bit`, `MatchTest12Bit`, `CheckOutput`, `CheckOutput2D`, `RunCheckOutput`, `DISABLED_speed`
- **Validates**: Palette mode color counting (LBD/HBD at multiple bit depths), K-means clustering, K-means indices computation
- **ISA variants**: AVX2, NEON

#### `AdaptiveScanTest.cc`

- **Fixture**: `CopyMiMapGridTest` + standalone TEST
- **Tests**: `scan_tables_test`, `test_match`
- **Validates**: Scan table ordering verification and MI map grid copy
- **ISA variants**: SSE2, AVX2

#### `FFTTest.cc` (x86 only)

- **Fixtures**: `FFT2DTest`, `IFFT2DTest`
- **Tests**: `run_fft_accuracy_check`, `run_fft_ifft_check`, `run_ifft_accuracy_check`
- **Validates**: 2D FFT and IFFT accuracy, round-trip (FFT then IFFT) correctness

#### `TemporalFilterTestPlanewise.cc`

- **Fixtures**: `TemporalFilterTestPlanewiseMedium`, `TemporalFilterZZTestPlanewiseMedium`, `TemporalFilterTestPlanewiseMediumHbd`, `TemporalFilterZZTestPlanewiseMediumHbd`, `TemporalFilterTestGetFinalFilteredPixels`, `TemporalFilterTestApplyFilteringCentralLbd`, `TemporalFilterTestApplyFilteringCentralHbd`, `EstimateNoiseTestFP`, `EstimateNoiseTestFPHbd`
- **Tests**: `OperationCheck`, `DISABLED_Speed`, `RunTest`, `fixed_point`
- **Validates**: Temporal filter planewise operations (LBD/HBD, regular and zero-zero modes), final filtered pixel computation, central filtering application, and noise estimation (fixed-point)
- **ISA variants**: AVX2, AVX512, NEON

### API Tests

#### `SvtAv1EncApiTest.cc`

- **Fixture**: None (standalone TESTs)
- **Tests**:
  - `EncApiDeathTest/set_parameter_null_pointer`: Validates null pointer handling in `svt_av1_enc_set_parameter`
  - `EncApiTest/check_null_pointer`: Validates all API functions reject null pointers with `EB_ErrorBadParameter`
  - `EncApiTest/DISABLED_check_normal_setup`: Normal init-set-init-deinit lifecycle
  - `EncApiTest/DISABLED_repeat_normal_setup`: 500x init/deinit cycle for leak detection

#### `SvtAv1EncParamsTest.cc`

- **Fixture**: `EncParamTestBase` (macro-generated subclasses via `DEFINE_PARAM_TEST_CLASS`)
- **Tests**: `run_paramter_check` (for each parameter class, which runs `run_default_param_check`, `run_valid_param_check`, `run_invalid_param_check`, `run_special_param_check`)
- **Parameters tested** (each is a separate test class):
  - `enc_mode`, `intra_period_length`, `intra_refresh_type`, `hierarchical_levels`, `pred_structure`
  - `source_width`, `source_height`, `encoder_bit_depth`
  - `qp`, `use_qp_file`, `enable_dlf_flag`, `film_grain_denoise_strength`
  - `rate_control_mode`, `scene_change_detection`, `target_bit_rate`
  - `max_qp_allowed`, `min_qp_allowed`
  - `level`, `use_cpu_flags`, `level_of_parallelism`, `recon_enabled`
  - `tile_columns`, `tile_rows` (conditional on `TILES`)
  - `screen_content_mode`, `enable_tf`, `enable_overlays`
  - `color_range`, `color_primaries`, `transfer_characteristics`, `matrix_coefficients`

#### `MultiEncoderTest.cc`

- **Fixture**: None (standalone TESTs)
- **Tests**:
  - `ConcurrentEncoders`: 2 encoder instances encoding 30 frames simultaneously on separate threads
  - `RepeatedInitDeinit`: 5 iterations of concurrent 2-encoder encoding, testing resource reference counting
  - `UnsynchronizedRecreation`: Concurrent encoders with staggered create/destroy cycles (3 cycles per thread)
  - `VaryingBlockGeometrySizes`: Concurrent encoders alternating between small (RTC, enc_mode=11) and large (enc_mode=0) block geometry configurations, testing global table allocation thread safety

### E2E Tests

#### `SvtAv1E2ETest.cc`

- **Fixture**: `CrashDeathTest` (extends `SvtAv1E2ETestFramework`)
- **Tests**: `NotCrashTest`
- **Validates**: Encoder does not crash with any preset (0 through MAX_ENC_PRESET) on default test vectors
- **Parameters**: All encoder presets x {kirland_640_480, niklas_640_480}

- **Fixture**: `ConformanceDeathTest` (extends `SvtAv1E2ETestFramework`)
- **Tests**: `DefaultSettingTest`
- **Validates**: Encoder reconstruction matches AOM reference decoder output (pixel-exact)
- **Covers**:
  - Encoder modes 0, 3, 5, 8
  - Intra period: -1, 10
  - QP: 0, 10, 20, 32, 44, 63
  - Deblocking filter: off, auto
  - Film grain: 0, 1, 6, 10, 50
  - Rate control: VBR
  - Scene change detection: off
  - Screen content: off, forced on
  - Adaptive quantization: mode 1
  - Temporal filter: off
  - Tiles: row=1, col=1, both
  - Overlays: on (with various presets)
  - Super resolution: modes 1-4, various denominator/threshold combinations
  - Reference scaling: fixed, random access
  - Dummy source (color bar)
  - Quantization matrices: on/off, various min/max levels

- **Fixture**: `OverlayPresetConformanceTest` (DISABLED)
- **Tests**: `DISABLED_OverlayPresetTest` - Overlay with all presets 0-10

- **Fixture**: `SuperResPresetConformanceTest` (DISABLED)
- **Tests**: `DISABLED_SupreResPresetTest` - Super-resolution with all presets 0-10

- **Fixture**: `SwitchFrameConformanceTest` (DISABLED)
- **Tests**: `DISABLED_SwitchFrameTest` - S-Frame feature with various intervals (16, 32, 64) x hierarchical levels (3, 4, 5) x prediction structures (Low Delay, Random Access) x presets (8, 9, 10)

- **Fixture**: `LongtimeConformanceTest` (DISABLED)
- **Tests**: `DISABLED_LongtimeTest` - Extended encoding from config file

- **Fixture**: `TileIndependenceTest`
- **Tests**: `TileTest` - Tiles decoded in inverted order still match reference decoder

- **Fixture**: `SuperResTest`
- **Tests**: `SuperResolutionTest` - Fixed super-resolution with all denom x kf_denom (9x9=81 combinations), Q-threshold mode, reference scaling fixed mode, combined super-res + ref scaling, random access scaling events

- **Fixture**: `FeaturePresetConformanceTest` (DISABLED)
- **Tests**: `DISABLED_FeaturePresetConformanceTest` - Cross-product of hierarchical levels x presets x feature values

- **Fixture**: `SegmentTest`
- **Tests**: `AqMode1Test` - Adaptive quantization mode 1 with various QP + preset + optional tiles, using 720p test vector

#### `SvtAv1E2EParamsTest.cc`

- **Fixture**: `CodingOptionTest` (extends `SvtAv1E2ETestFramework`)
- **Tests**: `CheckEncOptionsUsingBitstream`
- **Validates**: Encoder parameters actually take effect in the bitstream by:
  - Decoding with AOM reference decoder's analyzer mode
  - Checking profile, bit depth, color format, intra period, QP range, tile configuration match
- **Covers**: Intra period, QP values, rate control (VBR with various bitrates), min/max QP bounds

## Test Vectors

### Unit Test Vectors
Unit tests are self-contained -- they generate random input data and compare C-vs-SIMD output. No external test vectors needed.

### E2E Test Vectors

Downloaded via CMake target `TestVectors` from a URL defined in `test/e2e_test/test_vector_list.txt`. Checksums are verified. The download path is controlled by `SVT_AV1_TEST_VECTOR_PATH`.

Standard vectors:
- `kirland_640_480_30.yuv` - 640x480, 8-bit, YUV420, 60 frames (default) or 1000 frames (long)
- `niklas_640_480_30.yuv` - Same spec
- `park_joy_90p_8_420.y4m` - 160x90, 8-bit, Y4M (incomplete SB test)
- `park_joy_90p_10_420.y4m` - 160x90, 10-bit, Y4M
- `screendata.y4m` - 640x480, 8-bit, screen content
- `niklas_1280_720_30.y4m` - 1280x720, 8-bit (segment tests)

Synthetic vectors:
- `DummyVideoSource`: Color bar pattern generator for arbitrary resolutions (480p, 1080p, 64x64) at 8-bit and 10-bit

## Benchmarking System

The `test/benchmarking/` directory contains a Python-based codec comparison framework, separate from the Google Test infrastructure.

### Architecture

- `config_manager.py` - YAML configuration management with placeholder resolution
- `encode.py` - Parallel encoding across codecs, qualities, and speeds
- `decode_and_qm.py` - Decoding and quality metric computation
- `qm.py` - Quality metric calculation (VMAF, SSIMULACRA2, MS-SSIM via vmaf library)
- `bd_rate_utils.py` / `bd_metric.py` - Bjontegaard Delta rate computation
- `summary.py` - Report generation
- `analysis_and_plotting.py` - Chart and plot generation
- `format_conversion.py` - Image/video format conversion (PNG, YUV, Y4M)
- `data_readers.py` - XML/result file parsing
- `utils.py` - Shared utilities

### Workflow

1. Configure via YAML (dataset, codecs, metrics, output paths)
2. `run_comparison.sh <config>` orchestrates the full pipeline
3. Encode all combinations of codec x quality x speed in parallel
4. Decode and compute quality metrics
5. Generate BD-rate summary and plots

Supports multi-machine workflows where encoding, decoding, and summary can run on separate hosts.

### Speed Tests in Unit Tests

Many unit test fixtures include `DISABLED_Speed` or `DISABLED_SpeedTest` variants that measure function execution time. These run tight loops (typically 1000-10000 iterations) and report timing. They are disabled by default and must be explicitly enabled via `--gtest_also_run_disabled_tests` or filter selection.

## Coverage Gaps & Recommendations

### What is NOT Tested

1. **No ARM-specific test files**: The `arm_arch_files` list is empty. ARM SIMD tests exist only through `multi_arch_files` instantiations. Functions that are ARM-only (e.g., SVE2-specific algorithms) may lack test coverage.

2. **Several x86-only tests have no ARM counterparts**:
   - `noise_model_test.cc` - Noise model SIMD functions
   - `highbd_intra_prediction_tests.cc` - AVX512 highbd intra prediction
   - `FFTTest.cc` - FFT/IFFT operations
   - `ForwardtransformTests.cc` - Forward transform AVX512
   - `FwdTxfm1dTest.cc` / `InvTxfm1dTest.cc` - 1D transform accuracy
   - `FwdTxfm2dTest.cc` - 2D transform accuracy
   - `MotionEstimationTest.cc` - ME SAD functions
   - `PsnrTest.cc` - HBD PSNR
   - `frame_error_test.cc` - Frame error

3. **Missing encoder pipeline unit tests**: No tests for:
   - Rate control decision logic
   - Mode decision scoring
   - Partition search algorithms
   - Reference frame management
   - Temporal layer assignment
   - GOP structure generation
   - Lambda/QP derivation
   - Bitrate buffer model

4. **No fuzz testing**: No randomized/fuzz input to encoder API or internal functions.

5. **No bitstream conformance golden tests**: The E2E tests verify encode-decode round-trip, but there are no tests that check a specific input produces an exact bitstream output (determinism testing).

6. **No multi-pass encoding tests**: Two-pass and multi-pass encoding modes are not tested.

7. **Several E2E tests are DISABLED**: `OverlayPresetConformanceTest`, `SuperResPresetConformanceTest`, `SwitchFrameConformanceTest`, `LongtimeConformanceTest`, `FeaturePresetConformanceTest` are all disabled by default.

8. **No memory safety testing**: No AddressSanitizer, ThreadSanitizer, or MemorySanitizer integration in the test infrastructure.

9. **No boundary resolution tests in E2E**: Only 640x480, 160x90, 1280x720, and dummy 1920x1080/64x64 are tested. Missing odd resolutions, minimum/maximum dimensions, non-multiple-of-8 sizes.

10. **Limited chroma format coverage**: Only YUV420 is tested in E2E. YUV422 and YUV444 test vectors are commented out.

11. **No HDR/WCG tests**: No 10-bit or HDR content E2E tests (only 8-bit YUV files in default vectors, though 10-bit Y4M exists for sub-SB tests).

12. **No regression test suite**: No mechanism to detect performance regressions (encoding speed, compression efficiency) across commits.

### Recommendations for a Port

#### Testing Strategy

1. **Property-based testing over golden tests**: The existing C-vs-SIMD pattern is good for a port. Replace it with property-based tests that verify mathematical properties:
   - Forward + inverse transform round-trip within bounded error
   - SAD/SSE non-negativity and triangle inequality
   - Convolution linearity
   - Prediction boundary conditions

2. **Deterministic bitstream tests**: Add tests that verify a known input produces an exact bitstream (byte-for-byte). This catches subtle differences in rounding, ordering, or state management.

3. **Architecture-neutral reference implementations**: Port the C reference functions first, then test SIMD against them. The existing test infrastructure already separates C and SIMD -- preserve this.

4. **Comprehensive resolution testing**: Test all combinations of:
   - Widths/heights: 64, 65, 66, 128, 129, 130, 256, 640, 720, 1080, 1920, 2160, 3840, 4096
   - Non-multiple-of-SB sizes (not aligned to 64 or 128)
   - Minimum (64x64) and near-minimum sizes

5. **Sanitizer integration**: Build and test with ASan, TSan, and MSan as first-class CI targets.

6. **Encode-decode conformance with libaom**: Keep the pattern of encoding with the port and decoding with a reference AV1 decoder. This is the strongest correctness guarantee.

7. **Performance regression tracking**: Add CI benchmarks that track encoding speed (FPS) and compression efficiency (PSNR/bitrate) per commit. Flag regressions automatically.

8. **Fuzz the encoder API**: Use cargo-fuzz or similar to feed random parameter combinations and input data to the encoder.

#### Port-Specific Test Infrastructure

1. **Rust test framework**: Use `#[test]` with `proptest` for property-based testing. Use `criterion` for benchmarks.

2. **SIMD testing**: Use feature flags (`#[cfg(target_feature = "avx2")]`) to gate SIMD tests. Test the dispatch mechanism separately.

3. **FFI boundary tests**: If wrapping C SIMD, add tests at the FFI boundary to verify data layout and calling convention correctness.

4. **Thread safety tests**: Rust's type system helps, but add explicit multi-threaded encoding tests similar to `MultiEncoderTest`.

5. **Incremental porting validation**: For each module ported, add a cross-validation test that runs both the C and Rust implementations on the same input and compares output. Remove these once the port is complete.

#### Test Priority for Porting

Port tests in this order (dependencies first):

1. **Transform tests** - Foundation for everything else
2. **Quantization tests** - Required for rate-distortion
3. **Prediction tests** - Intra first, then inter
4. **SAD/Variance tests** - Required for motion estimation
5. **Convolution tests** - Required for inter prediction
6. **Filter tests** - Deblock, CDEF, restoration
7. **API tests** - Public interface contract
8. **E2E conformance** - Full pipeline validation
