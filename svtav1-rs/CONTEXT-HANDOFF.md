# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-17
**Lines:** ~25,000 | **Files:** 70 | **Tests:** 460 (71 golden/spec parity) | **Warnings:** 0

## What This Is

A safe Rust port of SVT-AV1 (Alliance for Open Media's fastest production AV1 encoder).
Located at `svtav1-rs/` within the SVT-AV1 C source tree. 8-crate workspace.

## What Works (verified, wired, exercised)

### Transforms — 100% complete, bit-exact with C
- 26/26 1D kernels (fdct/idct/fadst/iadst/fidentity/iidentity at sizes 4-64)
- All verified against C SVT-AV1 output via `tools/extract_golden.c` (51 golden tests)
- 19/19 TxSizes with 2D wrappers (square + rectangular)
- 16/16 TxType dispatch through `txfm_dispatch.rs`
- Mode-specific RDO: V→ADST-DCT, H→DCT-ADST, diagonal→ADST-ADST, DC/smooth→DCT-DCT

### Intra Encoding Pipeline — working end-to-end
The `EncodePipeline::encode_frame()` produces OBU bitstream output through this chain:
1. Temporal filtering (when reference frames available, gated by speed preset)
2. VAQ activity map → frame-level QP adjustment
3. SB-by-SB raster order partition search (spec 00 compliance)
   - 10/10 AV1 partition types (NONE/HORZ/VERT/SPLIT/H4/V4/HA/HB/VA/VB)
   - 11 intra modes + 5 filter-intra modes evaluated per block
   - 4 transform types in RDO (DCT-DCT/ADST-DCT/DCT-ADST/ADST-ADST)
   - Speed config gates partition types and candidate count by preset
   - **Frame-level reconstruction neighbors** from previously-finalized SBs (c824a6fe)
4. encode_block: transform → quantize → reconstruct at 4x4/8x8/16x16/32x32
5. Loop filter chain: deblocking → CDEF → Wiener → sgrproj (all 5 tools wired)
6. Film grain estimation (source vs reconstruction comparison)
7. Coefficient coding via write_coefficients (Exp-Golomb, not literals)
8. OBU bitstream: temporal_delimiter + sequence_header + frame OBU
9. DPB update with reconstruction, rate control state update

### Inter-Frame Encoding — basic working (4c90c56b)
- Full-pel ME (SAD-based, ±16 search) against most recent DPB reference
- Per-block intra vs inter RD comparison in partition search
- Inter prediction: full-pel copy from reference + MV
- MV rate overhead in RD cost computation
- All 10 partition types try inter when reference available

### DSP Layer — all tools implemented
- Intra: 21 modes (DC/V/H/smooth*/paeth/directional/CfL/filter-intra/palette/IntraBC)
- Inter: convolution/copy/blend/OBMC/warped/scaled (8-tap sub-pixel interpolation)
- Loop filter: deblock/CDEF/Wiener/sgrproj/super-res
- SIMD: archmage dispatch on all 10+ DSP modules (AVX2/NEON/scalar)

### Speed Presets — 11 of 20 flags actively used
14 presets (0-13) with 20 feature flags. 11 flags actively used in the pipeline:
enable_cdef, enable_restoration, enable_temporal_filter, max_partition_depth,
lambda_scale, enable_ext_partitions, enable_4to1_partitions, enable_directional,
enable_adst, rdo_tx_decision, enable_filter_intra

### Entropy Context — AV1 spec default CDFs (2a1691f3)
FrameContext initialized from spec Section 9.3 default tables:
partition, kf_y_mode, y_mode, skip, intra_inter, single_ref, comp_ref, comp_inter

### Conformance — OBU structure validated (160e6586)
3 OBU conformance tests: key frame structure (TD→SH→Frame), multi-frame sequence,
sequence header profile validation. Writes test bitstream for external dav1d testing.

## What's Simplified (code exists but not at full C quality)

- **OBMC**: blend masks real, doesn't compute neighbor predictions (spec 06)
- **Deblocking**: 4-tap only, missing 6/8/14-tap variants + strength derivation (spec 08)
- **Prediction neighbors**: within-SB sub-blocks use 128 (cross-SB fixed)
- **Entropy coding**: real Exp-Golomb coeff coding, but not CDF-based context-adaptive (spec 07)
- **OBU output**: valid for still-picture, simplified for inter frames (spec 07)
- **Inter ME**: full-pel only, no half-pel interpolation (simplified sub-pel)
- **sgrproj**: O(N*radius²) naive box sums instead of integral images
- **Wiener**: QP-based heuristic coefficients, not per-RU RDO optimization

## What's Not Implemented

- Full AV1 default CDF initialization tables for coefficient/TX CDFs
- CDF-based arithmetic entropy coding (currently Exp-Golomb)
- TPL (temporal propagation layer) for rate control
- Multi-threaded tile/segment parallelism
- Full inter-frame OBU headers (reference frame signaling, order hints)
- Full context derivation for partition/mode/reference syntax elements
- Sub-pel ME interpolation (half/quarter/eighth precision)
- Reference MV stack construction from spatial/temporal neighbors
- AV1 bitstream conformance (pass dav1d decode without errors)

## Key Files

| File | Purpose |
|------|---------|
| `svtav1-rs/STATUS.md` | Detailed per-spec coverage audit |
| `svtav1-rs/CLAUDE.md` | Project rules (TDD, archmage, safety) |
| `svtav1-rs/tools/extract_golden.c` | C program to extract golden test data |
| `svtav1-rs/svtav1/tests/golden_parity.rs` | 71 bit-exact parity tests |
| `svtav1-rs/svtav1/tests/e2e_correctness.rs` | 44 end-to-end tests |
| `svtav1-rs/svtav1/tests/real_encode.rs` | 18 real encoding + OBU tests |
| `svtav1-rs/crates/svtav1-dsp/src/fwd_txfm.rs` | All forward transforms (largest file) |
| `svtav1-rs/crates/svtav1-encoder/src/pipeline.rs` | Encoding pipeline orchestrator |
| `svtav1-rs/crates/svtav1-encoder/src/partition.rs` | Partition search with neighbors + inter |

## Build & Test

```bash
cd svtav1-rs
cargo test --workspace          # 460 tests
cargo clippy --workspace --all-targets  # 0 warnings
cargo run -p svtav1-dsp --features std --example perf_report --release  # benchmarks
```

## Priority for Next Session

1. **CDF-based arithmetic encoding** — replace Exp-Golomb with spec-conformant CDF coding using the default tables already in FrameContext
2. **Sub-pel ME refinement** — wire half_pel_refine into motion_est for better inter prediction quality
3. **Inter-frame OBU headers** — write proper frame headers with reference frame signaling for inter frames
4. **Within-SB neighbors** — pass recon data between sub-blocks in partition trials (Level 2 of neighbor threading)
5. **Reference MV stack** — derive MV context from spatial/temporal neighbors (enables better MV prediction)
6. **Multi-tap deblocking** — add 6, 8, 14-tap filter variants with proper strength derivation

## Rules (from CLAUDE.md and memory)

- `#![forbid(unsafe_code)]` on all crates except svtav1-cuda
- NEVER fabricate performance numbers — only report measured values
- NEVER claim completion unless code matches the spec it references
- Report gaps FIRST, progress second. Use fractions: "5 of 16" not "done"
- All transform ports must be verified bit-exact against C golden data
- Document rav1d-safe borrowings in CLAUDE.md "Borrowed Patterns"
