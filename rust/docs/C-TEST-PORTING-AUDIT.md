# C-test → Rust coverage audit (zenav1-svt)

**Scope of this audit:** compare the SVT-AV1 C unit/asm/e2e test suite
(`reference/svt-av1/test/`, **69** `.cc`/`.c` files) against the Rust port's test
surface, and produce an honest coverage map + prioritized gap list.

**Read-only analysis. No code was built or changed.** Generated 2026-07-24 by
inspection of both trees at `master`.

## Port scope (what "in scope" means here)

Per `rust/README.md` + `rust/CLAUDE.md`, the port is a **still-picture / all-intra**
reimplementation of SVT-AV1 v4.2.0, byte-identical on this envelope:
**8-bit 4:2:0, CQP, single frame (KEY), single-threaded, 64-aligned dims, `--lp 1`.**
10-bit and arbitrary dims are in-progress (bd10 gates + partial-SB gates exist).
A separate **HDR-fork** mode adds psy-RD / QM / photon-noise / variance-boost, verified
*functionally* (per-kernel differentials + decode gates), not byte-gated.

Consequences for classification:
- **Inter / motion-compensation / compound / global-motion / temporal-filter** kernels
  are **out of scope** (N/A) — a still KEY frame never runs them. Where the port still
  ships a *kernel* differential (e.g. `obmc_blend`, `convolve8`) it is noted, but the
  test's primary feature is inter.
- **Quality metrics** (PSNR / SSIM-metric / VMAF) are **output measurement**, not the
  encode path → N/A. (tune=SSIM's *mode-decision* distortion is separately covered.)
- **Decoder** tests: SVT's suite is encoder-only, so there are none to port.

## Coverage vocabulary

- **PORTED (differential)** — a `c_parity_*` test calls the **real exported C function**
  and asserts byte/word equality against the Rust port. Strongest bar.
- **PORTED (e2e-covered)** — no direct unit differential, but the kernel is on the
  byte-identity encode path exercised by the identity gates (`tools/identity_matrix.sh`
  132/132 synthetic + `real_image_matrix.sh` / `photo_p0_gate.sh` real content), so a
  divergence would surface as a stream mismatch. Plus inline `#[test]`s where noted.
- **N/A (out of scope)** — feature the port deliberately does not implement (with the
  cited reason).
- **GAP** — kernel is in the port's scope/path but has **no** Rust differential and
  (for the higher-priority ones) no dedicated inline/e2e assertion isolating it.

---

## 1. Full map (all 69 C test files)

### 1a. Transforms / quant / entropy (core intra path)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `FwdTxfm2dAsmTest.cc` | `svt_av1_fwd_txfm2d_*` SIMD | `c_parity_txfm.rs::fwd_txfm2d_matches_c` (:36), `::fwd_dispatch_rect_matches_c` (:247) | PORTED-diff (ASM tiers) |
| `FwdTxfm2dTest.cc` | fwd 2D txfm (all sizes/types) | `c_parity_txfm.rs::fwd_txfm2d_matches_c` (:36) | PORTED-diff |
| `ForwardtransformTests.cc` | `svt_av1_transform_two_d_*` wrappers | `c_parity_txfm.rs::fwd_named_square_wrappers_match_c` (:169), `::fwd_named_rect_wrappers_match_c` (:218) | PORTED-diff |
| `FwdTxfm1dTest.cc` | 1D fwd txfm primitives | subsumed by 2D differential (2D calls 1D) — `c_parity_txfm.rs` | PORTED-diff (indirect) |
| `FwdTxfm2dApproxTest.cc` | *approximate* fwd 2D txfm | none found | **GAP** (verify used at any preset) |
| `InvTxfm2dAsmTest.cc` | inv 2D txfm SIMD | `c_parity_txfm.rs::inv_txfm2d_recon_matches_c` (:78), `::inv_named_rect_wrappers_recon_match_c` (:307) | PORTED-diff (ASM tiers) |
| `InvTxfm1dTest.cc` | 1D inv txfm primitives | subsumed by inv-2D differential — `c_parity_txfm.rs` | PORTED-diff (indirect) |
| `QuantAsmTest.cc` | `quantize_b` / `highbd_quantize_b` SIMD | `c_parity_quant.rs::quantize_b_matches_c_dispatched` (:240); `c_parity_bd10_quant.rs` | PORTED-diff (ASM tiers) |
| `quantize_func_test.cc` | `quantize_fp` / `quantize_fp_qm` | `c_parity_quant.rs::quantize_fp_matches_c_dispatched` (:228); `c_parity_qm.rs::qm_quantize_matches_c_with_transcribed_tables` (:48) | PORTED-diff |
| `EncodeTxbAsmTest.cc` | `txb_init_levels`, `get_nz_map_contexts` SIMD | `svtav1-entropy/tests/c_parity.rs::txb_init_levels_simd_matches_c`, `::nz_map_contexts_simd_matches_c` | PORTED-diff (ASM tiers) |
| `BitstreamWriterTest.cc` | `od_ec_encode_bool_q15`, default coef probs | `svtav1-entropy/tests/c_parity.rs::ec_adaptive_stream_matches_c`, `::ec_carry_torture_matches_c`, `::c_default_cdf_tables_match`, `::update_cdf_matches_c` | PORTED-diff |
| `AdaptiveScanTest.cc` | `svt_copy_mi_map_grid` (mi-map bookkeeping) | none (port has its own mode-info grid) | **GAP** (low; internal grid copy) |

### 1b. Prediction (intra) / CfL

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `intrapred_dr_test.cc` | directional `dr_prediction_z1/z2/z3` | `c_parity_intra_edge.rs::dr_prediction_kernels_match_c` (:97) | PORTED-diff |
| `intrapred_edge_filter_test.cc` | `filter_intra_edge`, `upsample_intra_edge`, edge strength | `c_parity_intra_edge.rs::filter_intra_edge_matches_c` (:56), `::upsample_intra_edge_matches_c` (:76), `::edge_strength_and_upsample_decisions_match_c` (:32) | PORTED-diff |
| `highbd_intra_prediction_tests.cc` | HBD dc/v/h/paeth/smooth | `c_parity_intra_pred_hbd.rs::hbd_intra_predictors_match_c` (:123) | PORTED-diff |
| `FilterIntraPredTest.cc` | `filter_intra_predictor` (recursive FILTER_INTRA) | `c_parity_filter_intra.rs::filter_intra_predictor_matches_c` (:52) | PORTED-diff |
| `intrapred_test.cc` | **LBD** dc/v/h/paeth/smooth (8-bit) | impl `svtav1-dsp/src/intra_pred.rs`; inline `dc_*`/`paeth_*`/`smooth_*` (:1111–1225); e2e identity gates | **GAP** (no direct C-differential; HBD twin has one) |
| `intrapred_cfl_test.cc` | CfL predict + luma subsample (420) | impl `intra_pred.rs::cfl_predict_lbd/hbd` (:1069/:836), `cfl_luma_subsampling_420` (:1020); inline `cfl_ac_producers_agree_*` (:8681); e2e 4:2:0 gates | **GAP** (no direct C-differential; on hot 4:2:0 path) |
| `subtract_avg_cfl_test.cc` | CfL `subtract_average` | impl `intra_pred.rs::cfl_subtract_average` (:1042); e2e | **GAP** (no direct C-differential) |

### 1c. In-loop filters / restoration (all-intra: LF + CDEF opt, Wiener LR)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `DeblockTest.cc` | `lpf_horizontal/vertical_{4,6,8,14}` (+HBD) | `c_parity_lpf.rs::lpf_kernels_match_c_over_level_sharpness_space` (:121), `::lf_thresholds_match_c` (:34); `c_parity_lpf_hbd.rs::hbd_lpf_kernels_match_c_*` (:88) | PORTED-diff |
| `CdefTest.cc` | `cdef_filter_block`, `cdef_find_dir`, `compute_cdef_dist` | `c_parity_cdef.rs::filter_block_8bit_matches_c` (:342), `::filter_block_dispatch_all_tiers_match_c` (:619), `::find_dir_matches_c` (:82), `::compute_cdef_dist_8bit_matches_c` (:557); pick: `c_parity_cdef_pick.rs` (:13) | PORTED-diff (ASM tiers) |
| `wiener_convolve_test.cc` | `wiener_convolve_add_src` (+HBD) | `c_parity_wiener.rs::wiener_convolve_matches_c` (:67); `c_parity_wiener_hbd.rs::highbd_wiener_convolve_matches_c` (:82) | PORTED-diff |
| `RestorationPickTest.cc` | LR search stats (`compute_stats`, proj error) | `c_parity_wiener.rs::compute_stats_matches_c` (:133), `::compute_stats_all_tiers_match_c` (:188), `::filter_unit_matches_c` (:273); encoder search `svtav1-encoder/src/restoration.rs` | PORTED-diff (Wiener). **SGR proj-error N/A** |
| `selfguided_filter_test.cc` | `apply_selfguided_restoration` (SGR) | — | **N/A** — sgrproj is never searched (`svtav1-encoder/src/restoration.rs:8`; C ENC_MR=-1 stills) |
| `SelfGuidedUtilTest.cc` | SGR `pixel_proj_error`, `get_proj_subspace` | — | **N/A** — same (SGR not searched on stills) |

### 1d. Distortion / SAD / variance / hadamard (mode decision)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `hadamard_test.cc` | `hadamard_8x8/16x16/32x32` (+HBD) | `c_parity_hadamard.rs::hadamard_{8,16,32}_matches_c` (:80/85/90), `::hadamard_16x16_matches_avx2_8bit_range` (:153) | PORTED-diff (ASM tiers) |
| `SatdTest.cc` | `svt_aom_satd` | `c_parity_hadamard.rs` (uses `cref::satd`) | PORTED-diff |
| `SadTest.cc` | `svt_aom_sad*` (all sizes) | `c_parity_sad.rs::sad_matches_c_all_sizes_random` (:60), `::sad_matches_c_extremes` (:78) | PORTED-diff |
| `sad_neon_test.cc` | `svt_aom_sad*` NEON tier | `c_parity_sad.rs` (Rust SIMD tiers) | PORTED-diff (ASM tier) |
| `VarianceTest.cc` | `variance*`, `sub_pixel_variance*` | `c_parity_variance.rs::sse_matches_c_variance_sse_output` (:59), `::single_block_variance_matches_c_derived_numerator` (:78) | PORTED-diff (variance). sub-pel-variance = inter → N/A |
| `HbdVarianceTest.cc` | `variance_highbd` | `c_parity_hbd_distortion.rs::hbd_variance_matches_c` (:69); `c_parity_variance.rs` | PORTED-diff |
| `SpatialFullDistortionTest.cc` | `spatial_full_distortion_kernel`, cbf-zero, `full_distortion_kernel32` | `c_parity_tx_bias.rs::facade_matches_c_for_intra_modes` (:56, `cref::spatial_facade`); `c_parity_ssim_md.rs::ssim_md_kernel_matches_c` (:24) | PORTED-diff (spatial + ssim variants). `_kernel32`/`cbf_zero` variants = **partial GAP** (low) |
| `compute_mean_test.cc` | `compute_sub_mean_8x8`, mean-of-squares 8x8 | `c_parity_sb_qindex.rs` (`cref::sub_mean_8x8`, `cref::sub_mean_squared_8x8`) | PORTED-diff |
| `BlockErrorTest.cc` | `svt_av1_block_error_c` (coeff-domain SSE) | none (not referenced by port) | **GAP / N/A** — port distorts spatially; verify unused |
| `ResidualTest.cc` | `residual_kernel8/16bit` (source − pred) | `svtav1-encoder/src/leaf_funnel.rs` (inline); e2e identity gates | **GAP** (low; trivial, e2e-heavy) |
| `FullDistortionPerfTest.cc` | `full_distortion_kernel32` (perf) | `c_parity_hbd_distortion.rs::hbd_full_distortion_matches_c` (:46, kernel16) | N/A-perf (32-bit variant = partial GAP) |
| `VariancePerfTest.cc` | variance (perf) | perf: `tools/perf_gate.sh`; kernel: `c_parity_variance.rs` | N/A-perf |
| `SubpelVariancePerfTest.cc` | subpel variance (perf, inter) | — | N/A (inter + perf) |

### 1e. IntraBC / palette / screen-content (in scope, low presets)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `HashTest.cc` | `crc32c` (block hashing) | `c_parity_intrabc_hash.rs` (`cref::crc32c`, `::generate_block_hash`) | PORTED-diff |
| `IntraBcUtilTest.cc` | `is_dv_valid` | `c_parity_intrabc.rs::` (`cref::is_dv_valid`) | PORTED-diff |
| `PaletteModeUtilTest.cc` | `count_colors`, `k_means_dim1/2` | `c_parity_palette.rs::count_colors_matches_c` (:46), `::k_means_dim1_matches_c` (:169), `::calc_indices_dim1_matches_c` (:135) | PORTED-diff |

### 1f. Film-grain / noise (HDR-fork synthesis)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `FilmGrainTest.cc` | grain synthesis + `denoise_and_model` | `c_parity_noise_gen.rs::noise_table_matches_c` (:57); `c_parity_noise_norm.rs` | PORTED-diff (synthesis). **denoise-analysis = N/A** |
| `FFTTest.cc` | FFT (denoise pre-analysis) | — | **N/A** — denoise-analysis path not ported (fork uses ISO-strength synthesis) |
| `noise_model_test.cc` | `flat_block_finder`, noise model fit | — | **N/A** — same (denoise-analysis not ported) |

### 1g. Inter / motion / compound / global-motion / temporal (OUT OF SCOPE)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `MotionEstimationTest.cc` | inter ME (`sad*x4d`) | intrabc ME: `c_parity_intrabc_search.rs::c_parity_diamond_search` (:265), `::c_parity_full_pixel_search` (:348) | N/A-inter (intrabc ME covered) |
| `TemporalFilterTestPlanewise.cc` | temporal filter (multiframe) | `c_parity_temporal.rs` (`estimate_noise_fp16` only) | N/A (single-frame) |
| `GlobalMotionUtilTest.cc` | `ransac` (global motion) | — | N/A-inter |
| `corner_match_test.cc` | GM corner/feature matching | — | N/A-inter |
| `frame_error_test.cc` | GM frame error | — | N/A-inter |
| `warp_filter_test.cc` | `warp_affine` (warped motion) | `c_parity_warp.rs` — **witness: `warp.rs` is a STUB**, pins divergence | N/A-inter (stub witnessed) |
| `warp_filter_test_util.cc` | warp test harness (support) | — | infra/harness (counted with infra, not N/A) |
| `CompoundUtilTest.cc` | compound diffwtd mask, `obmc_mask` | `c_parity_obmc.rs::obmc_blend_above_matches_c` (:36) (blend only) | N/A-inter (obmc-blend kernel diff exists) |
| `WedgeUtilTest.cc` | wedge SSE / sign | — | N/A-inter |
| `OBMCSadTest.cc` | OBMC sad | — | N/A-inter |
| `OBMCVarianceTest.cc` | OBMC variance; `obmc_blend` | `c_parity_obmc.rs::obmc_blend_left_matches_c` (:58) (blend only) | N/A-inter (obmc-blend kernel diff exists) |
| `convolve_test.cc` | `jnt_convolve*` (compound) | — | N/A-inter |
| `Convolve8Test.cc` | `convolve8_horiz/vert` | `c_parity_inter_pred.rs::convolve_horiz_matches_c_all_phases` (:48), `::convolve_vert_matches_c_all_phases` (:67) | PORTED-diff (kernel; not on primary intra path) |
| `Convolve2dSrPerfTest.cc` | single-ref subpel convolve (perf) | — | N/A-inter/perf |
| `av1_convolve_scale_test.cc` | `convolve_2d_scale` (ref scaling) | `c_parity_scale.rs` — **witness: `scale.rs` is a STUB**, pins divergence | N/A-inter (stub witnessed) |
| `ResizeTest.cc` | `resize_plane` (down/up-scale) | `c_parity_superres.rs` (related) — stubbed | **GAP** (superres/resize; out of 64-aligned envelope) |

### 1h. Superres (still-picture tool, currently stubbed)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| *(no dedicated C file; via `ResizeTest` + normative upscale)* | `av1_convolve_horiz_rs` / `resize_filter_normative` | `c_parity_superres.rs` — **witness: `superres.rs` is a STUB**, pins divergence | **GAP** (in-scope still tool; superres defaults off, out of current envelope) |

### 1i. Metrics / packing / infra (N/A)

| C test file | Kernel under test | Rust coverage | Class |
|---|---|---|---|
| `PsnrTest.cc` | PSNR (output metric) | — | N/A-metric |
| `ssim_test.cc` | `svt_ssim_8x8/4x4` (output metric) | `c_parity_ssim_md.rs` (MD distortion, different use) | N/A-metric (MD-ssim covered) |
| `VmafTest.cc` | VMAF (output metric) | — | N/A-metric |
| `PackUnPackTest.cc` | MSB pack/unpack (10-bit layout) | — | N/A (port keeps 10-bit as native `u16`) |
| `MemTest.cc` | `copy_wxh_8/16bit` | — | N/A (Rust native slice copy) |
| `PictureOperatorTest.cc` | `downsample_2d` (depth/ME) | — | N/A (not used on single-frame intra) |
| `svt_av1_test.cc` | gtest main | — | N/A-infra |
| `TestEnv.c` | test env / rtcd setup | — | N/A-infra |
| `unit_test_utility.c` | random-buffer utilities | — | N/A-infra |

---

## 2. Summary counts

Excluding the 3 pure-infra files (`svt_av1_test.cc`, `TestEnv.c`, `unit_test_utility.c`)
and `warp_filter_test_util.cc` (harness support) → **65 behavioral C test files**:

| Class | Count |
|---|---|
| **PORTED (differential)** | **31** |
| PORTED (e2e-covered only, in the GAP rows) | — (folded into GAPs) |
| **N/A (out of scope)** | **26** |
| **GAP (in-scope, no differential)** | **8** |

Infra/harness (not behavioral): **4** (`svt_av1_test`, `TestEnv`, `unit_test_utility`,
`warp_filter_test_util`).

The port additionally has **differentials with no direct C-test twin** (extra coverage):
`c_parity_sc_detect` (screen-content detection), `c_parity_var_boost` (fork variance
boost), `c_parity_tx_bias` / `c_parity_ac_bias` (fork psy), `c_parity_sb_qindex`,
`c_parity_motion_est`, `c_parity_intrabc_mvp` / `_search`, `c_parity_mv` /
`c_parity_lr_syntax` (entropy MV + LR syntax coding).

---

## 3. Prioritized GAP list

Ranked by whether the kernel is actually exercised on the **allintra-KEY-4:2:0-8bit**
primary path (the port's byte-identity bar). Every "GAP" below is already
**e2e-exercised** by the identity gates — the gap is the *absence of an isolating
differential*, which means a divergence shows up as a whole-stream mismatch instead of a
pinpointed kernel failure.

### HIGH — on the primary hot path, no isolating differential

1. **CfL predict + luma-subsample + subtract-average**
   (`intrapred_cfl_test.cc`, `subtract_avg_cfl_test.cc`)
   - Kernel: `svt_cfl_predict_lbd/hbd`, `cfl_luma_subsampling_420_lbd/hbd`,
     `subtract_average`. **On the 4:2:0 all-intra hot path** (every chroma block that
     picks UV_CFL_PRED).
   - Now: implemented (`intra_pred.rs:836/1020/1042/1069`) + inline producer tests + e2e.
   - **Close it:** add `c_parity_cfl.rs` differentialing the port's CfL against
     exported `svt_cfl_predict_lbd_c` / `svt_cfl_luma_subsampling_420_lbd_c` /
     `svt_subtract_average` (the shim would need to expose them — cref does not today).

2. **LBD basic intra predictors (dc/v/h/paeth/smooth, 8-bit)** (`intrapred_test.cc`)
   - Kernel: 8-bit DC/PAETH/SMOOTH/V/H. **On the primary path** (luma intra every block).
   - Now: impl `intra_pred.rs`; inline self-checks (`dc_*`, `paeth_*`, `smooth_*`); e2e.
     The **HBD** twin *is* differential (`c_parity_intra_pred_hbd`), so only the LBD side
     lacks a C differential.
   - **Close it:** extend `c_parity_intra_pred_hbd.rs` (or add `c_parity_intra_pred_lbd`)
     to differential the 8-bit predictors vs the exported `svt_aom_*_predictor_*_c`.

### MEDIUM — on-path but simple, or a variant of a covered kernel

3. **`full_distortion_kernel32` / `full_distortion_kernel_cbf_zero`**
   (`SpatialFullDistortionTest.cc`, `FullDistortionPerfTest.cc`)
   - `spatial_facade` and the 16-bit kernel are differentialed; the **32-bit** coeff-domain
     and cbf-zero variants are not explicitly isolated.
   - **Close it:** add cases to `c_parity_hbd_distortion.rs` calling
     `svt_full_distortion_kernel32_bits_c` / `svt_full_distortion_kernel_cbf_zero32_bits_c`.

4. **`FwdTxfm2dApproxTest` — approximate forward transform**
   - Verify whether the approx-fwd-txfm speed path is reached at any tested preset. If it
     is, differential it; if the port never selects it, reclassify N/A.

5. **`ResidualTest` — `residual_kernel8/16bit`**
   - Trivial (source − pred), computed inline in `leaf_funnel.rs`, e2e-heavy. Low risk.
   - **Close it:** a small `c_parity_residual.rs` vs `svt_residual_kernel8bit_c`.

### LOW — off the primary path or bookkeeping

6. **`BlockErrorTest` — `svt_av1_block_error_c`**
   - Coeff-domain SSE; **not referenced anywhere in the port**. Either genuinely unused
     (port distorts spatially → N/A) or a latent RDOQ-distortion gap. **Verify usage
     first**; only add a differential if the port actually needs the kernel.

7. **`AdaptiveScanTest` — `svt_copy_mi_map_grid`**
   - Internal mode-info grid replication. The port has its own grid representation, so the
     C kernel likely has no analog. Verify the port's grid stamping is otherwise tested;
     probably reclassify N/A.

8. **Superres / resize** (`ResizeTest.cc`, superres normative upscale)
   - `superres.rs` / `scale.rs` are **documented stubs** (`c_parity_superres.rs`,
     `c_parity_scale.rs` pin the divergence with `assert_ne!`). Still-picture superres is
     an in-scope *feature*, but it defaults off and is outside the current 64-aligned
     envelope. When superres/arbitrary-dims lands, the witness `assert_ne!` flips to
     `assert_eq!` (per those files' own headers) — that is the built-in close signal.

---

## 4. ASM-only C tests → Rust SIMD-tier differentials

Several C tests exist purely to prove **SIMD == C-scalar** (`*AsmTest`, `*_neon_test`).
The port's equivalent is its own "all tiers identical" differential — the Rust kernel is
run through every dispatch tier and each is compared to the exported C scalar:

| C ASM test | Rust SIMD-tier differential |
|---|---|
| `FwdTxfm2dAsmTest.cc` | `c_parity_txfm.rs::fwd_dispatch_rect_matches_c` (:247) |
| `InvTxfm2dAsmTest.cc` | `c_parity_txfm.rs::inv_named_rect_wrappers_recon_match_c` (:307) |
| `QuantAsmTest.cc` | `c_parity_quant.rs::quantize_{b,fp}_matches_c_dispatched` (:228/240) |
| `EncodeTxbAsmTest.cc` | `svtav1-entropy/tests/c_parity.rs::txb_init_levels_simd_matches_c`, `::nz_map_contexts_simd_matches_c` |
| `CdefTest.cc` (SIMD tiers) | `c_parity_cdef.rs::filter_block_dispatch_all_tiers_match_c` (:619) |
| `hadamard_test.cc` (avx2) | `c_parity_hadamard.rs::hadamard_16x16_matches_avx2_8bit_range` (:153) |
| `sad_neon_test.cc` | `c_parity_sad.rs` (Rust archmage tiers vs `svt_aom_sad*_c`) |
| `wiener_convolve_test.cc` (tiers) | `c_parity_wiener.rs::compute_stats_all_tiers_match_c` (:188) |

**Note:** the port dispatches SIMD via `archmage` and differentials each tier against the
**C scalar** reference — it does **not** reproduce every C hand-written intrinsic variant
(AVX-512 / SVE / NEON-i8mm etc.) 1:1. That is the correct target: the contract is
"port tier == C scalar," identical to what the C `*AsmTest` asserts for its own SIMD.

---

## 5. Bottom line

The **differential coverage of the DSP/transform/quant/entropy/filter kernels on the
all-intra path is strong** (31 direct differentials, every ASM test mapped to a tier
differential). The real holes are a small, well-defined set:

- **CfL** and **LBD basic intra** are the only *hot-path* kernels lacking an isolating
  C-differential (both are heavily e2e-covered and have an HBD or inline twin, so the risk
  is low but the gap is real). These are the two worth closing first.
- Everything else is either an off-path/inter kernel (correctly N/A), a metric, a
  documented stub with a self-flipping witness (superres/scale/warp), or a trivial
  inline-computed kernel (residual). None of these threaten the byte-identity bar today.

No test was found to be *falsely* claimed as ported: the differentials cited above all
call the real exported C functions via `svtav1-cref`.
