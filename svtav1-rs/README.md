# svtav1-rs

A work-in-progress pure Rust AV1 encoder, ported from Intel's SVT-AV1.

**26k lines | 8 crates | 500+ tests | `#![forbid(unsafe_code)]` | BSD-2-Clause**

> **Status: experimental.** Many content/size/speed/quality combinations produce corrupt bitstreams that fail to decode. Not ready for production use. The zenavif integration is currently disabled pending further work on decode conformance.

### What works

- 128x128 gradient images at low speed (4) encode and decode correctly
- All 10 AV1 partition types, including extended T-shapes and 4:1
- 13 intra prediction modes with directional angle delta
- 16 transform types across 19 sizes, all bit-exact with C SVT-AV1
- CDF-based entropy coding matching the rav1d decoder
- Speed presets 0-13 with progressive feature gating

### What doesn't work yet

Many configurations produce corrupt bitstreams:
- Small images (< 128x128) at most speed presets
- High speed presets (8+) at most sizes
- Uniform/all-skip content (zero coefficients)
- Quality 70 with certain gradient patterns

The root causes are CDF context interaction bugs in the coefficient encoder and frame header generation. Diagnosing these requires decoder-side range coder state tracing.

## Quick start

```rust
use svtav1::avif::AvifEncoder;

let pixels: Vec<u8> = make_your_image(width, height);
let encoder = AvifEncoder::new()
    .with_quality(70.0)   // 1-100 (higher = better quality, larger file)
    .with_speed(8);       // 1-10 (higher = faster, lower quality)

let result = encoder.encode_y8(&pixels, width, height, width)?;
// result.data contains a complete AV1 OBU bitstream
```

For YUV 4:2:0:

```rust
let result = encoder.encode_yuv420(&y, &u, &v, width, height, stride)?;
```

## Architecture

Eight focused crates, minimal external dependencies (only archmage for SIMD dispatch):

```
svtav1                  Public API, AVIF backend
  svtav1-encoder        Pipeline, partition search, mode decision, rate control
    svtav1-dsp          SIMD transforms, prediction, filtering (archmage)
    svtav1-entropy      Range coder, CDF tables, OBU serialization
    svtav1-tables       Const lookup tables, scan orders
    svtav1-types        Core AV1 type definitions
    svtav1-disjoint-mut Region-based borrow tracking
  svtav1-cuda           Optional GPU bridge (stub)
```

### Encoding pipeline

```
encode_frame(y_plane)
  temporal_filter (if inter + refs available)
  activity_map (VAQ QP adjustment)
  for each 64x64 superblock (raster order):
    partition_search (tries up to 10 partition types)
      encode_single_block at each leaf:
        evaluate 11 intra modes with TX-type RDO
        transform, quantize, reconstruct
  deblock (4/8/14-tap per edge)
  CDEF (8x8 directional filter)
  Wiener + sgrproj restoration
  entropy coding (CDF-based, write_coefficients_v2)
  OBU bitstream output
```

The coefficient encoder (`write_coefficients_v2`) matches rav1d's exact bitstream reading order: CDF-based EOB bin + hi-bit, reverse diagonal scan, separate token phases for base/BR/signs, and Golomb residual coding. Default CDFs are extracted from rav1d for all 4 QP categories.

### Speed presets

| Preset | Partition depth | Intra modes | Transform types | Loop filters |
|--------|-----------------|-------------|-----------------|--------------|
| 0-3    | 4 (down to 4x4) | 13 (all)   | All 16          | All enabled  |
| 4-6    | 3               | 7           | DCT + ADST      | All enabled  |
| 7-9    | 2               | 4           | DCT + ADST      | Deblock + CDEF |
| 10-13  | 1 (64x64 only)  | 2 (DC + V)  | DCT only        | Deblock only |

The `speed` parameter on `AvifEncoder` (1-10) maps linearly to presets 0-13.

## Building

Requires Rust 1.85+ (2024 edition).

```bash
cargo build --workspace
cargo test --workspace          # 500+ tests, ~15s
cargo clippy --workspace        # 0 warnings
```

The `justfile` provides shortcuts:

```bash
just test      # cargo test --workspace
just ci        # fmt + clippy + test (local sanity check)
just bench     # cargo bench --workspace
```

## Testing

All 26 forward and inverse 1D transform kernels are verified bit-exact against C SVT-AV1 golden output, extracted via `tools/extract_golden.c`. Transform parity covers DCT (4-64), ADST (4-16), and identity (4-64).

Decode conformance is tested through [zenavif](https://github.com/imazen/zenavif)'s differential tests, which encode with svtav1-rs and decode with rav1d-safe.

```bash
# Run decode conformance tests (requires zenavif checkout)
cd /path/to/zenavif
cargo test --features "encode,encode-svtav1" --test differential_svtav1
cargo test --features "encode,encode-svtav1" --test differential_comprehensive
```

## Known bugs

1. **Content-specific decode failures at q70** — Most quality levels decode correctly. Some specific QP/content combinations fail, likely due to a CDF adaptation interaction with coefficient density. Requires decoder-side range coder state tracing to diagnose.

2. **All-skip frames fail** — Uniform content where all coefficients quantize to zero produces undecodable bitstreams. Root cause undiagnosed.

3. **Some multi-SB sizes at certain speeds** — 80x80, 96x96, 112x112 fail at some speed presets despite 128x128 working.

## Safety

Every crate uses `#![forbid(unsafe_code)]` except `svtav1-cuda` (FFI boundary, isolated). SIMD dispatch goes through archmage's token system, which generates safe code from `#[arcane]`/`#[rite]` annotations.

The `svtav1-disjoint-mut` crate provides region-based borrow tracking for concurrent superblock encoding, adapted from [rav1d-safe](https://github.com/memorysafety/rav1d)'s `rav1d-disjoint-mut`. Our version is simplified (no UnsafeCell, fully safe).

## License

BSD-2-Clause. Same license as the original SVT-AV1.

## Acknowledgments

- [SVT-AV1](https://gitlab.com/AOMediaCodec/SVT-AV1) (Intel/Alliance for Open Media) — the C encoder this port is based on
- [rav1d-safe](https://github.com/memorysafety/rav1d) — safe Rust AV1 decoder used for conformance testing; DisjointMut pattern borrowed
- [archmage](https://github.com/nickelc/archmage) — SIMD dispatch via CPU feature tokens
