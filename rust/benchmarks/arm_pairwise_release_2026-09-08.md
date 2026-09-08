# PR #20: published pairwise widening verification

Integrated PR head `a9eeb58e` with main `25708931`. Keep main's registry-only
archmage, archmage-macros and magetypes **0.9.29** and unchanged lockfile.
The former Git patch is no longer needed. Published source includes u8x16
`abs_diff`, `sum_abs_diff`, and `pairwise_widen_add`, plus u16x8
`pairwise_widen_add`. The implementation uses widening sums to preserve lane
accumulation, the final reduction, strides and remainder handling.

## Local checks

Host: x86_64 Linux; rustc 1.98. All heavy commands were serialized with the
shared run-heavy wrapper (16 GiB, four jobs). Cargo used `--locked`.

- Native full workspace nextest: **2631/2631 passed, zero skipped**.
- AArch64 DSP library cross-check: passed against the registry packages.
- AArch64 benchmark `kernel_tiers` cross-check: passed, including the frozen
  references and row-packing experiment. Existing C-reference build warnings
  about promoted statics remain.
- AArch64 DSP library nextest under QEMU 10.2 (`-cpu max`): **19/19 selected
  tests passed**. The explicit filter `test(me_sad) | test(variance)` deselected
  237 unrelated library tests. This executes NEON dispatch permutations,
  odd widths, unequal strides, scalar tails and maximum-difference cases.
- Scoped Rust formatting and `git diff --check`: passed.
- Strict AArch64 library Clippy: **17 pre-existing diagnostics remain**, matching
  the archived original PR's library result. Unchanged files account for the
  doc/style diagnostics; the untouched dotprod arm accounts for the two
  incompatible-MSRV diagnostics (declared 1.89, intrinsics stabilized 1.98).
  No lint suppression or test expectation was changed to hide these.

The initial all-target cross-build could not link C parity executables because
its oracle archive is native x86_64. The successful QEMU command explicitly
uses `--lib`; it is not a claim of cross-compiled C-oracle coverage. Native
workspace tests include the C parity integrations.

Reproduce (prefix each command with the shared run-heavy wrapper):

```sh
cargo nextest run --workspace --locked --test-threads 4
cargo check --locked -p zenav1-svt-dsp --lib --target aarch64-unknown-linux-gnu
cargo check --locked -p zenav1-svt-dsp --bench kernel_tiers --target aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER='qemu-aarch64 -cpu max -L /usr/aarch64-linux-gnu' \
cargo nextest run --locked --target aarch64-unknown-linux-gnu -p zenav1-svt-dsp --lib -E 'test(me_sad) | test(variance)' --test-threads 4
cargo clippy --locked -p zenav1-svt-dsp --lib --target aarch64-unknown-linux-gnu --no-deps -- -D warnings
```

These are correctness and build checks. QEMU timings are not performance
measurements. The retained M4 Pro results in `arm_pairwise_2026-09-07.meta`
remain historical evidence of equivalent ME instruction bodies and no
meaningful throughput gain. No new speed, RD or native ARM codegen result
is claimed for the registry release. CI was not awaited.
