# SVT-AV1 Rust Port Rules

## CONFORMANCE MANDATE

**NEVER stop working while ANY conformance or parity issue remains.** If the bitstream does not decode with rav1d-safe at ALL tested sizes, the work is NOT DONE. If any differential test shows a decode failure, investigate the root cause and fix it before committing documentation, before updating handoffs, before doing anything else. Conformance failures are the #1 priority — above new features, above performance, above code cleanup. Do not describe a conformance failure as "expected" or "known" — describe it as "BLOCKING" and fix it in the same session.

This applies to:
- Bitstream decode failures at any image size
- Parity mismatches between C golden output and Rust implementation
- CDF/entropy coding mismatches that produce non-decodable output
- OBU structure errors that cause decoder rejection

**The definition of "done" for any encoding feature is: rav1d-safe decodes the output correctly at all tested sizes.**

## Commit Discipline
- **Commit after EVERY meaningful change.** After porting a function, commit. After adding a test, commit. After fixing a test, commit. Never batch more than ~30 minutes of work into one commit.
- **Push after every commit.** CI runs on remote.
- **Commit message format:** `<type>(<crate>): <description>` — e.g., `feat(svtav1-dsp): port forward DCT 4x4 with AVX2`

## Safety Rules
- `#![forbid(unsafe_code)]` on every crate by default.
- Archmage `#[arcane]`/`#[rite]` do NOT require `#[allow(unsafe_code)]` — they generate safe code via tokens.
- NEVER add `unsafe` without: (1) profiling evidence, (2) `cargo asm` evidence, (3) feature-flag gating, (4) parity test, (5) comment citing benchmark commit.
- NEVER use `core::arch::*` directly — only `archmage::prelude::*`.
- NEVER `#[inline(always)]` on `#[arcane]`/`#[rite]` functions.

## TDD Rules
- Write the test BEFORE the implementation for every ported function.
- Every function must have a parity test against C golden output.
- Every SIMD function must pass `for_each_token_permutation`.
- Floating-point: exact match required. Don't accept "small differences" — find the divergence.
- Run `cargo test` before every commit.

## Archmage Rules
- `#[arcane]` = entry points only (after token summon). One per hot path.
- `#[rite]` = inner helpers (no dispatch overhead). Default for all SIMD helpers.
- `incant!` for multi-platform dispatch with `[v3, neon, scalar]` tiers.
- Token summon once at entry, pass through call chain.
- `Desktop64` for x86 AVX2+FMA, `NeonToken` for ARM, `ScalarToken` always.

## Porting Rules
- Read the ENTIRE C function before porting. Don't port line-by-line — understand the algorithm.
- Port ALL constants/tables/helpers. No stubs, no TODOs.
- Reference `specs/` files for algorithm documentation.
- Exact-match parity test before moving to next function.
- Use `specs/18-testing.md` for test patterns and coverage requirements.
- Floating-point determinism: test pipeline stages independently.

## Performance Rules
- Correctness first, performance second. Get parity, THEN optimize.
- Profile with `cargo flamegraph` and `cargo asm` before optimizing.
- SIMD via archmage only — never hand-write `asm!` blocks.
- Benchmark with criterion/divan. Commit results to `benchmarks/`.
- Thread-local scratch buffers for ME/transform intermediates (rav1d-safe pattern).

## Spec References
- `specs/00-architecture.md` through `specs/18-testing.md` are the algorithm bible.
- When in doubt, read the spec, then read the C source, then implement.
- Update specs if you discover they're wrong.

## Reference Code
- rav1d-safe at `/home/lilith/work/zen/rav1d-safe/` can be referenced for patterns (DisjointMut, archmage usage, etc.)
- Document any borrowing from rav1d-safe in commit messages and here under "Borrowed Patterns"

## Borrowed Patterns
- **DisjointMut concept**: The `svtav1-disjoint-mut` crate's region-based borrow tracking
  pattern is adapted from `rav1d-disjoint-mut` at `/home/lilith/work/zen/rav1d-safe/crates/rav1d-disjoint-mut/`.
  Our implementation is simplified (no UnsafeCell, fully safe), but the API shape
  (Region, BorrowTracker, overlap detection) follows rav1d's design.

## Known Bugs — BLOCKING

1. **Coefficient context divergence at high quality** — At q70+ (QP ≤ 19), dense coefficient blocks fail to decode. Lower quality (q30-q60) decodes successfully. Root cause: likely a remaining context derivation mismatch in base_tok/br_tok that accumulates over many non-zero coefficients. The default CDF initialization may also be slightly off for TX type CDFs (currently uniform, should use rav1d spec defaults).

2. **All-skip frames fail to decode** — When all blocks have eob=0 (uniform content, very high QP), the frame structure may be missing required elements. Manifests as "direct 64x64" (23 bytes) and "uniform 128x128" (25 bytes) failing.

### Fixed Bugs (this session)
- **Spec-conformant coefficient encoder (write_coefficients_v2)** — Complete rewrite: CDF-based EOB (bin + hi-bit + equi), reverse diagonal scan, proper token structure (eob_base_tok/base_tok/br_tok), DC sign after tokens, AC signs separate, Golomb in sign phase, default CDFs from rav1d for 4 QP categories.
- **Coefficient stride mismatch for 64x64 blocks** — AV1 caps to 32x32 scan but coefficients use original stride. Fixed with scan_to_coeff_idx conversion.
- **TX type signaling for blocks < 32x32** — Decoder expects explicit TX type symbol; now writes DCT_DCT (index 1) via txtp_intra CDF.
- **Golomb residual in wrong phase** — Was written during token phase, moved to sign/residual phase matching rav1d.
- **PARTITION_HORZ/VERT children wrote extra partition symbols** — Fixed: children encode block syntax directly.
- **Partition context always sub=0 for multi-SB** — Fixed: rav1d-compatible partition context tracking.
- **Extended partition types had missing trees** — Fixed: all 6 types now build proper trees.

## Investigation Notes

### Transform Parity
All 26 1D transform kernels are bit-exact with C SVT-AV1. Verified by extracting golden data from C object files (`cbuild/Source/Lib/Codec/CMakeFiles/CODEC.dir/transforms.c.o` and `inv_transforms.c.o`). The C functions accept `(input, output, cos_bit, stage_range)` — we pass `cos_bit=12` and `stage_range=NULL` for forward (ignored), `wide_range=[31;12]` for inverse (clamping never triggers at 8-bit).

Key finding: the C `svt_av1_fadst4_new` uses i32 arithmetic while our initial port used i64, producing different rounding. Fixed by matching the C decomposition exactly. Same issue with `fadst8` output permutation — C uses `[step[1], step[6], step[3], step[4], step[5], step[2], step[7], step[0]]` without negation, while our initial port had sign flips.

### Pipeline Architecture
The pipeline processes superblocks in raster order (left-to-right, top-to-bottom) per spec 00. Each SB goes through partition_search which recursively tries all 10 partition types. At each leaf, encode_single_block evaluates 11 intra modes with mode-specific TX RDO, picking the lowest RD cost. Loop filters (deblock → CDEF → Wiener → sgrproj) are applied frame-wide after all SBs are encoded.

### Performance
Release-mode benchmarks (x86_64 AVX2):
- SAD 16x16: ~18 Gpix/s (archmage auto-vectorization)
- fwd_txfm 4x4: ~170 Mpix/s
- fwd_txfm 8x8: ~215 Mpix/s
These numbers are MEASURED, not estimated.
