# Native ARM audit, 2026-09-06

Performance changes are blocked by the required baseline gate. DSP measurements and the regression spot-check have not yet run.

On Apple M4 Pro / macOS 26.5.2 / Rust 1.98, production `878dd0be`, `cargo nextest run --workspace --build-jobs 4 --test-threads 4` ran 1172 of 2550 tests: 1171 passed, one failed, and nextest cancelled the remaining 1378. The failure is `pd0::pd0_quant_parity_tests::pd0_quantize_b_matches_c_all_tiers`, qindex 0 / transform 0 / pattern 2 / C dispatch enabled. The retained excerpt has exact EOB, qcoeff and dqcoeff differences.

The test at `crates/svtav1-encoder/src/pd0.rs:846` feeds values including ±131072 and −32768 to both C scalar and C runtime dispatch and requires equality with both. Rust agrees with the independent scan reference and C scalar before failing against C dispatch. The C shim `crates/svtav1-cref/shims/ref_shims.c:1391` calls the actual runtime pointer. On ARM, `reference/svt-av1/Source/Lib/Codec/aom_dsp_rtcd.c:634` binds it to NEON; `ASM_NEON/mem_neon.h:1273` narrows i32 to i16 with `vmovn_s32`, and `ASM_NEON/av1_quantize_neon.c:660` takes signed-16-bit absolute values before the threshold comparison. These oracles cannot both define the same output for the tested wide coefficients.

No test expectations, production quantizer behavior, or C reference sources have been changed. Approval was requested to keep full-range C-scalar parity, add an explicit oracle-divergence regression, and separately check C-NEON equality over a producer-verified domain. The producer bound has not yet been established by this audit; the existing comments in `tests/c_parity_quant.rs` are not treated as proof.

The existing `crates/svtav1-dsp/benches/kernel_tiers.rs` has 13 token toggles inside timed loops. Its final transform section also calls the C-exact driver without pairing token states, although `fwd_txfm2d_c_exact` and its inverse have SIMD fast paths in `txfm_simd`. Those benchmark corrections remain unimplemented pending the baseline gate.

Test discovery initially spent time before Rust entry: a one-second sample of a `--list` child showed only `_dyld_start`, with 96 KiB physical footprint. The build finished in 1m49s. An unnecessary restart followed a mistaken interpretation of nextest `-j`; this version aliases it to `--test-threads`, so the original four-thread test cap was already correct. All logs are preserved.
