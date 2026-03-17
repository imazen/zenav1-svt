# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-17
**Lines:** ~28,000 | **Files:** 70 | **Tests:** 460 | **Warnings:** 0

## What This Is

A safe Rust port of SVT-AV1 (Alliance for Open Media's fastest production AV1 encoder).
Located at `svtav1-rs/` within the SVT-AV1 C source tree. 8-crate workspace.

## Encoding Pipeline — Full Feature Summary

### Input → Encode → Bitstream
1. **Temporal filtering** — multi-reference weighted average (gated by speed preset)
2. **VAQ activity map** → frame-level QP adjustment
3. **SB-by-SB partition search** with frame-level + within-SB neighbor context
   - 10/10 AV1 partition types (NONE/HORZ/VERT/SPLIT/H4/V4/HA/HB/VA/VB)
   - 11 intra modes + 5 filter-intra modes
   - **Inter prediction**: full-pel SAD → half-pel → quarter-pel ME with bilinear interpolation
   - **Spatial MV prediction**: median-of-3 from above/left/diagonal neighbor MVs
   - Per-block intra vs inter RD comparison across all partition types
   - 4 TX types in RDO (DCT-DCT/ADST-DCT/DCT-ADST/ADST-ADST)
   - Gated by 11 speed config flags
4. **encode_block**: transform → quantize → reconstruct (4x4-32x32)
5. **Loop filter chain**:
   - Deblocking: 4-tap (inner) + 8-tap (moderate) + 14-tap (SB boundary) with QP-adaptive strength
   - CDEF: direction-based, QP-scaled strengths
   - Wiener: **per-frame coefficient optimization** (minimizes SSE vs source)
   - sgrproj: **O(1) per pixel via integral images**
6. Film grain estimation
7. **CDF-based entropy coding**: coefficient CDFs with backward-adaptive context per frame
8. **OBU bitstream**:
   - Key frames: TD + SH (reduced or full) + Frame OBU
   - Inter frames: Frame OBU with proper header (type, QP, refresh_flags, ref indices, order_hint)
   - Full (non-reduced) sequence header for multi-frame sequences with enable_order_hint

### Transforms — 100% complete, bit-exact with C
- 26/26 1D kernels, 19/19 2D sizes, 16/16 TX types

### DSP Layer — All Tools Implemented
- Intra: 21 modes (DC/V/H/smooth*/paeth/directional/CfL/filter-intra/palette/IntraBC)
- Inter: 8-tap convolution, copy, blend, OBMC, **8-tap sub-pixel warped/scaled**
- SIMD: archmage dispatch (AVX2/NEON/scalar) on all modules

### Entropy — AV1 Spec Default CDFs + Adaptive Coding
- FrameContext from spec Section 9.3 (partition, kf_y_mode, y_mode, skip, refs, etc.)
- CDF-based encoding functions: partition, skip, intra_inter, intra_mode (kf + inter)
- CDF-based coefficient coding (txb_skip, dc_sign, base_level, base_range)
- Backward-adaptive CDF updates per-frame

### Speed Presets — 11 of 20 flags active
enable_cdef, enable_restoration, enable_temporal_filter, max_partition_depth,
lambda_scale, enable_ext_partitions, enable_4to1_partitions, enable_directional,
enable_adst, rdo_tx_decision, enable_filter_intra

## Remaining Gaps to Production

### Critical for bitstream conformance
- **Tile OBU structure**: current tile data is raw coded coefficients without proper tile_start_and_end/tile_header syntax
- **Proper key frame header for non-reduced SH**: key frame header assumes reduced_still_picture_header but full SH needs different frame header
- **Context derivation**: partition/mode contexts use simplified indices instead of spec-defined neighbor-based context derivation
- **Coefficient coding context**: txb_skip/base_level contexts use simplified position-based indexing

### Quality improvements
- **OBMC neighbor predictions**: blend masks exist but pipeline doesn't generate neighbor block predictions
- **MV map recording**: spatial MVP reads from zero-initialized map; needs per-block MV recording during encode

### Performance
- **Multi-threaded encoding**: no tile/segment parallelism
- **TPL rate control**: no temporal propagation layer

## Build & Test

```bash
cd svtav1-rs
cargo test --workspace          # 460 tests
cargo clippy --workspace --all-targets  # 0 warnings
```

## Key Changes This Session (24 feature commits)

| Area | What |
|------|------|
| Prediction neighbors | Frame-level + within-SB (HORZ/VERT) neighbor context |
| Speed config | 3 new flags: enable_adst, rdo_tx_decision, enable_filter_intra |
| Inter encoding | Full-pel → half-pel → quarter-pel ME with spatial MVP |
| Sub-pixel interp | 8-tap for warped/scaled, bilinear for ME/prediction |
| Deblocking | 4/8/14-tap filters with QP-adaptive strength derivation |
| sgrproj | O(1) integral images replacing O(N*r²) box sums |
| Wiener | Per-frame coefficient optimization (SSE minimization) |
| Entropy | CDF-based coding for coefficients + partition/mode/skip syntax |
| OBU | Full SH with order_hint, inter frame headers with ref signaling |
| Conformance | 3 OBU structure validation tests |
