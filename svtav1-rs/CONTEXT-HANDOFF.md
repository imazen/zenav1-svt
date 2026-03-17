# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-17
**Lines:** ~26,000 | **Files:** 70 | **Tests:** 460 | **Warnings:** 0

## What This Is

A safe Rust port of SVT-AV1 (Alliance for Open Media's fastest production AV1 encoder).
Located at `svtav1-rs/` within the SVT-AV1 C source tree. 8-crate workspace.

## What Works (verified, wired, exercised)

### Transforms — 100% complete, bit-exact with C
- 26/26 1D kernels (fdct/idct/fadst/iadst/fidentity/iidentity at sizes 4-64)
- 19/19 TxSizes with 2D wrappers, 16/16 TxType dispatch
- Mode-specific RDO gated by enable_adst and rdo_tx_decision speed flags

### Intra Encoding Pipeline — working end-to-end
1. Temporal filtering (gated by speed preset)
2. VAQ activity map → frame-level QP adjustment
3. SB-by-SB partition search with **frame-level + within-SB neighbors**
   - 10/10 AV1 partition types
   - 11 intra modes + 5 filter-intra modes (gated by enable_filter_intra)
   - 4 transform types in RDO (gated by enable_adst + rdo_tx_decision)
   - HORZ/VERT partition trials pass recon borders to sibling sub-blocks
4. encode_block: transform → quantize → reconstruct at 4x4-32x32
5. Loop filter: deblocking (4-tap + **8-tap wide** at SB boundaries) → CDEF → Wiener → sgrproj
6. Film grain estimation
7. **CDF-based coefficient coding** with backward-adaptive context per frame
8. OBU bitstream: TD + SH + Frame OBU

### Inter-Frame Encoding — working
- **Half-pel ME** (full-pel SAD search + bilinear half-pel refinement) against DPB reference
- Per-block intra vs inter RD comparison across all 10 partition types
- **Bilinear-interpolated** inter prediction at sub-pel positions
- Proper inter frame OBU headers (frame type, QP, refresh flags, ref indices)
- DPB update with reconstruction

### DSP Layer — all tools implemented
- Intra: 21 modes including filter-intra and directional
- Inter: convolution/copy/blend/OBMC/warped/scaled (**8-tap sub-pixel interpolation**)
- Loop filter: deblock (4-tap + **8-tap wide**)/CDEF/Wiener/sgrproj/super-res
- SIMD: archmage dispatch on all 10+ DSP modules

### Speed Presets — 11 of 20 flags actively used
enable_cdef, enable_restoration, enable_temporal_filter, max_partition_depth,
lambda_scale, enable_ext_partitions, enable_4to1_partitions, enable_directional,
enable_adst, rdo_tx_decision, enable_filter_intra

### Entropy — AV1 spec default CDFs + CDF-based coding
- FrameContext initialized from spec Section 9.3 default tables
- Coefficient coding uses CDF-based arithmetic with adaptive updates
- Shared CoeffContext persists across blocks within each frame

### Conformance — OBU structure validated
3 tests: key frame structure, multi-frame sequence, SH profile validation

## What's Simplified

- **OBMC**: blend masks real, doesn't compute neighbor predictions
- **Deblocking**: 4-tap + 8-tap only, missing 6/14-tap + proper strength derivation per-edge
- **Entropy**: CDF-based for coefficients, but literal for partition/mode/skip syntax
- **OBU**: simplified inter frame headers (no order_hint, error_resilient=1)
- **sgrproj**: O(N*radius²) naive box sums instead of integral images
- **Wiener**: QP-based heuristic coefficients, not per-RU RDO
- **ME**: half-pel bilinear only, no quarter/eighth sub-pel

## What's Not Implemented

- CDF-based encoding for partition type, intra mode, skip, inter mode syntax
- Full context derivation for partition/mode/reference syntax elements
- Quarter/eighth-pel ME interpolation
- Reference MV stack (spatial/temporal neighbor MV derivation)
- TPL (temporal propagation layer) for rate control
- Multi-threaded tile/segment parallelism
- Non-reduced sequence header for proper multi-frame bitstreams
- AV1 bitstream conformance (pass dav1d decode without errors)

## Build & Test

```bash
cd svtav1-rs
cargo test --workspace          # 460 tests
cargo clippy --workspace --all-targets  # 0 warnings
```

## Priority for Next Session

1. **CDF-based syntax coding** — extend CDF encoding to partition type, intra mode, skip (currently literal)
2. **Reference MV stack** — spatial/temporal neighbor MV derivation for better inter prediction
3. **Non-reduced sequence header** — enable proper multi-frame bitstream support
4. **Quarter-pel ME** — extend sub-pel refinement to quarter and eighth precision
5. **sgrproj integral images** — O(1) per-pixel box sums
6. **Wiener RDO** — per-restoration-unit coefficient optimization
