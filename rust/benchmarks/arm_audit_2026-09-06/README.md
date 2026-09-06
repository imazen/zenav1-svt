# Native ARM audit, 2026-09-06

The baseline blocker is resolved by `dd9d51d4`: 2556/2556 tests and 104/104 whole-encoder regression cells pass. See [oracle-resolution.md](oracle-resolution.md) for the C scalar versus NEON decision and its measured limits. DSP measurement is now in progress. The sections below preserve the earlier baseline investigation.

On Apple M4 Pro / macOS 26.5.2 / Rust 1.98, production `878dd0be`, `cargo nextest run --workspace --build-jobs 4 --test-threads 4` ran 1172 of 2550 tests: 1171 passed, one failed, and nextest cancelled the remaining 1378. The failure is `pd0::pd0_quant_parity_tests::pd0_quantize_b_matches_c_all_tiers`, qindex 0 / transform 0 / pattern 2 / C dispatch enabled. The retained excerpt has exact EOB, qcoeff and dqcoeff differences.

The test at `crates/svtav1-encoder/src/pd0.rs:846` feeds values including ±131072 and −32768 to both C scalar and C runtime dispatch and requires equality with both. Rust agrees with the independent scan reference and C scalar before failing against C dispatch. The C shim `crates/svtav1-cref/shims/ref_shims.c:1391` calls the actual runtime pointer. On ARM, `reference/svt-av1/Source/Lib/Codec/aom_dsp_rtcd.c:634` binds it to NEON; `ASM_NEON/mem_neon.h:1273` narrows i32 to i16 with `vmovn_s32`, and `ASM_NEON/av1_quantize_neon.c:660` takes signed-16-bit absolute values before the threshold comparison. These oracles cannot both define the same output for the tested wide coefficients.

No test expectations, production quantizer behavior, or C reference sources have been changed. Approval was requested to keep full-range C-scalar parity, add an explicit oracle-divergence regression, and separately check C-NEON equality over a producer-verified domain. The producer bound has not yet been established by this audit; the existing comments in `tests/c_parity_quant.rs` are not treated as proof.

The existing `crates/svtav1-dsp/benches/kernel_tiers.rs` has 13 token toggles inside timed loops. Its final transform section also calls the C-exact driver without pairing token states, although `fwd_txfm2d_c_exact` and its inverse have SIMD fast paths in `txfm_simd`. Those benchmark corrections remain unimplemented pending the baseline gate.

Test discovery initially spent time before Rust entry: a one-second sample of a `--list` child showed only `_dyld_start`, with 96 KiB physical footprint. The build finished in 1m49s. An unnecessary restart followed a mistaken interpretation of nextest `-j`; this version aliases it to `--test-threads`, so the original four-thread test cap was already correct. All logs are preserved.

## Whole-encoder regression spot-check

After syncing through concurrent `e1a555f4`, the existing `tools/regression_spotcheck.sh` passed **104 / 104**, with no skips ([log](svt-regression-spotcheck.log)). The script checks previously fixed encoder regressions, including C byte parity, explicit refusals and independently decoded reconstruction. `RS_AOMDEC=/opt/homebrew/bin/aomdec`, `SCREEN_DIR=/Users/lilith/work/zen/codec-corpus/gb82-sc` and `CID22_DIR=/Users/lilith/work/zen/codec-corpus/CID22/CID22-512` supplied all optional inputs.

Command: `/opt/homebrew/bin/bash tools/regression_spotcheck.sh`, from `rust/`, under nice -n19 with four build/Rayon/OMP/test threads and `TMPDIR=/Users/lilith/tmp`. The Rust identity example and C capture driver were built first with full output retained ([Rust build](svt-spotcheck-rust-build.log), [C build](svt-spotcheck-c-build.log)). On macOS the driver uses its existing byte-only fallback because the linker has no GNU `--wrap`; no op-level trace coverage is claimed. The C source remained unchanged.

This verifies those 104 encoder regression cells. It does not prove a universal transform-coefficient bound or resolve the quantizer unit-test disagreement. Concurrent V3 Hadamard and DC-fill changes are not work from this ARM audit.

## Full no-fail-fast follow-up

At production `e1a555f4`, `cargo nextest run --workspace --build-jobs 4 --test-threads 4 --no-fail-fast` ran all **2553 tests: 2552 passed, one failed, zero skipped**. The only failure is the same `pd0_quantize_b_matches_c_all_tiers` C-NEON disagreement, again qindex 0 / transform 0 / pattern 2. The completed test phase took 62.171 s; this excludes the 1m29s build and macOS test-discovery startup delays. [Summary](full_suite_summary.log).

No production optimization or test-expectation change was made. This closes the untested remainder from the original fail-fast run while retaining the explicit quantizer blocker. The C source is still `39f909e0` with no working-copy changes.

## Corrected kernel baseline

`kernel_tiers.rs` moves 13 token-toggle sites out of timed bodies and adds
untimed scalar/native equality checks to value kernels and nine transform
shapes. A full run took 286.4 seconds (53 noisy rounds). Many CVs exceed 20%;
paired intervals and raw results are retained in [svt-kernel-tiers.log](svt-kernel-tiers.log).

Three small-block cases lose: SATD4 native/scalar 47.2/41.5 ns, SSE4
53.9/49.8 ns, SSE8 134/130 ns. Their paired scalar deltas are −12.0% to
−9.1%, −9.5% to −6.4%, and −5.4% to −2.0%, respectively. Larger vector
kernels favor NEON in this run. The final quantize group compares different
coefficient counts; its ratio is not a SIMD comparison and must not be used
as one. Source optimization of the three losing cases is under investigation.
