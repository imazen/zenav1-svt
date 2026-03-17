# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-16
**Lines:** 23,345 | **Files:** 70 | **Tests:** 471 (71 golden/spec parity) | **Warnings:** 0

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
   - 11 intra modes evaluated per block (DC/V/H/smooth*/paeth/6 directional)
   - 4 transform types in RDO (DCT-DCT/ADST-DCT/DCT-ADST/ADST-ADST)
   - Speed config gates partition types and candidate count by preset
4. encode_block: transform → quantize → reconstruct at 4x4/8x8/16x16/32x32
5. Loop filter chain: deblocking → CDEF → Wiener → sgrproj (all 5 tools wired)
6. Film grain estimation (source vs reconstruction comparison)
7. Coefficient coding via write_coefficients (Exp-Golomb, not literals)
8. OBU bitstream: temporal_delimiter + sequence_header + frame OBU
9. DPB update with reconstruction, rate control state update

### DSP Layer — all tools implemented
- Intra: 21 modes (DC/V/H/smooth*/paeth/directional/CfL/filter-intra/palette/IntraBC)
- Inter: convolution/copy/blend/OBMC/warped/scaled (simplified — see below)
- Loop filter: deblock/CDEF/Wiener/sgrproj/super-res
- SIMD: archmage dispatch on all 10+ DSP modules (AVX2/NEON/scalar)

### Speed Presets
14 presets (0-13) with 20 feature flags. 8 flags actively used in the pipeline:
enable_cdef, enable_restoration, enable_temporal_filter, max_partition_depth,
lambda_scale, enable_ext_partitions, enable_4to1_partitions, enable_directional

## What's Simplified (code exists but not at full C quality)

- **Warped motion**: nearest-neighbor sampling, not 8-tap sub-pixel (spec 06)
- **Scaled prediction**: nearest-neighbor, not filtered (spec 06)
- **OBMC**: blend masks real, doesn't compute neighbor predictions (spec 06)
- **Deblocking**: 4-tap only, missing 6/8/14-tap variants + strength derivation (spec 08)
- **Prediction neighbors**: partition search uses mid-gray (128) instead of reconstruction neighbors (spec 05)
- **Entropy coding**: real Exp-Golomb coeff coding, but not CDF-based context-adaptive (spec 07)
- **OBU output**: valid for still-picture, simplified for inter frames (spec 07)

## What's Orphaned (code + tests exist, not called by pipeline)

These all require inter-frame encoding to be meaningful:
- `svtav1_dsp::obmc`, `warp`, `scale`, `intrabc`, `superres` — inter DSP tools
- `svtav1_encoder::motion_est` — full-pel + half-pel search
- `svtav1_encoder::multipass` — first-pass stats collection
- `svtav1_entropy::mv_coding` — class-based MV coding
- `svtav1_entropy::tile` — multi-tile OBU format
- `svtav1_encoder::perceptual` — QM, trellis quantization (VAQ is wired)

## What's Not Implemented

- Full AV1 default CDF initialization tables (~400 lines of const data from spec)
- TPL (temporal propagation layer) for rate control
- Multi-threaded tile/segment parallelism
- Full inter-frame OBU headers (reference frame signaling, order hints)
- 8-tap warped/scaled prediction interpolation
- Full context derivation for partition/mode/reference syntax elements
- AV1 bitstream conformance testing (decode output with a reference decoder)

## Key Files

| File | Purpose |
|------|---------|
| `svtav1-rs/STATUS.md` | Detailed per-spec coverage audit |
| `svtav1-rs/CLAUDE.md` | Project rules (TDD, archmage, safety) |
| `svtav1-rs/tools/extract_golden.c` | C program to extract golden test data |
| `svtav1-rs/svtav1/tests/golden_parity.rs` | 71 bit-exact parity tests |
| `svtav1-rs/svtav1/tests/e2e_correctness.rs` | 44 end-to-end tests |
| `svtav1-rs/svtav1/tests/real_encode.rs` | 15 real encoding tests with PSNR |
| `svtav1-rs/crates/svtav1-dsp/src/fwd_txfm.rs` | All forward transforms (largest file) |
| `svtav1-rs/crates/svtav1-encoder/src/pipeline.rs` | Encoding pipeline orchestrator |
| `svtav1-rs/crates/svtav1-encoder/src/partition.rs` | Partition search with 10 types |

## Build & Test

```bash
cd svtav1-rs
cargo test --workspace          # 471 tests
cargo clippy --workspace --all-targets  # 0 warnings
cargo run -p svtav1-dsp --features std --example perf_report --release  # benchmarks
```

## C Golden Data Extraction

```bash
cd cbuild
gcc -O0 -g ../svtav1-rs/tools/extract_golden.c \
  Source/Lib/Codec/CMakeFiles/CODEC.dir/transforms.c.o \
  Source/Lib/Codec/CMakeFiles/CODEC.dir/inv_transforms.c.o \
  -lm -o /tmp/extract_golden
/tmp/extract_golden  # prints golden values
```

## Priority for Next Session

1. **Fix prediction neighbors** — pass reconstruction buffer through partition search so blocks read from previously-encoded neighbors instead of mid-gray. This is the single biggest quality gap.
2. **Wire remaining speed config flags** — 12 of 20 unused (enable_adst, enable_cfl, enable_filter_intra, enable_palette, enable_compound, rdo_tx_decision, max_intra_candidates, subpel_precision, hme_levels, me_search_width, me_search_height, enable_warped_motion)
3. **Add inter-frame encoding** — wire motion_est + mv_coding + inter candidates into pipeline for non-key frames. This unlocks all orphaned modules.
4. **Fix warped motion** — replace nearest-neighbor with 8-tap sub-pixel interpolation
5. **Add AV1 default CDF tables** — initialize FrameContext from spec tables instead of uniform
6. **Conformance testing** — decode our OBU output with rav1d-safe to verify bitstream validity

## Rules (from CLAUDE.md and memory)

- `#![forbid(unsafe_code)]` on all crates except svtav1-cuda
- NEVER fabricate performance numbers — only report measured values
- NEVER claim completion unless code matches the spec it references
- Report gaps FIRST, progress second. Use fractions: "5 of 16" not "done"
- All transform ports must be verified bit-exact against C golden data
- Document rav1d-safe borrowings in CLAUDE.md "Borrowed Patterns"
