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

1. **Tile data does not decode under the reference decoder (aomdec): coefficient
   writer uses wrong tables + wrong context indexing** — 0/525 conformance-matrix
   streams decode. Symbol-trace diff (tools: `symtrace` feature + gdb script on a
   debug aomdec) shows the first stream divergence at symbol op 14: encoder used
   eob_base_tok CDF [602,250] where the decoder expects [1903,120] — both rows
   exist in the C defaults at *different coordinates*. The old `coeff.rs` tables
   are flattened/partially-uniform inventions. Fix underway: C-exact tables now
   live in `default_cdfs.rs` (generated from libSvtAv1Enc.a); the writer must be
   re-ported from C `av1_write_coeffs_txb_1d` (entropy_coding.c:448) with C
   context derivation (`svt_av1_get_nz_map_contexts`, `get_br_ctx`,
   `svt_aom_get_txb_ctx`).

2. **Prior "decode PASS" claims were rav1d-leniency artifacts** — the old zenavif
   rav1d-safe checks accepted streams the reference decoder rejects (and "PASS
   PSNR 11.1 dB" was garbage output). aomdec via `tools/decode_conformance.sh`
   is now the decode gate. Do not trust pre-2026-07-13 conformance claims.

3. **Monochrome is a dead end for C parity** — C SVT-AV1 hardcodes
   `is_monochrome = 0` (write_color_config); it cannot emit mono streams. The
   Rust pipeline is currently luma-only and signals mono_chrome=1 (spec-legal,
   now parses after the color_range fix). For bit-identity the pipeline must
   encode 4:2:0 with chroma planes + chroma syntax. Structural work item.

4. **Filter signaling inconsistency** — the encoder pipeline applies
   deblock/CDEF/restoration to its recon but signals them OFF in headers
   (enable_cdef=0, lf levels 0). Decoder recon therefore diverges from encoder
   recon (the "PSNR 11 dB" garbage). Either signal the filters or stop applying
   them until the C-faithful filter-signaling port lands.

### Fixed this wave (2026-07-13, wave2/entropy-c-parity)
- Range coder + update_cdf are now exact C ports, proven byte-identical by
  differential fuzz vs libSvtAv1Enc.a (`svtav1-cref` harness, tests/c_parity.rs).
- SH: monochrome color_config now writes the required color_range bit.
- FH: disable_cdf_update always signaled; delta_q_present gated on qidx>0.
- OBU_FRAME: FH ends with zero byte_alignment (was trailing-one).
- Tile group: no TG header bits for single tile; zero-align for multi.
- Default CDFs: all coefficient+mode tables extracted from C into
  `default_cdfs.rs` with a drift test pinning them to the linked library.

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
