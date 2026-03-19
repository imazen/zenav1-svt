# svtav1-rs

A pure Rust AV1 encoder, ported from Intel's SVT-AV1. Produces spec-conformant AV1 bitstreams that decode with rav1d/dav1d.

**26k lines | 8 crates | 500+ tests | `#![forbid(unsafe_code)]` | BSD-2-Clause**

## What it does

Encodes still images and video sequences as AV1 bitstreams. Designed primarily as a backend for [zenavif](https://github.com/imazen/zenavif) (AVIF image encoding), where it's available behind the `encode-svtav1` feature flag:

```toml
[dependencies]
zenavif = { version = "0.1", features = ["encode", "encode-svtav1"] }
```

The encoder handles multi-frame sequences with inter prediction, temporal filtering, and proper GOP structure. It produces OBU-formatted bitstreams with correct sequence headers, frame headers, tile groups, and entropy-coded coefficient data. Tested against rav1d-safe (the safe Rust AV1 decoder) for decode conformance.

### What works

- Single-SB frames (up to 64x64) decode at quality levels 30-60 and 90
- Multi-SB frames (128x128, 2x2 superblocks) decode
- All 10 AV1 partition types, including extended T-shapes and 4:1
- 13 intra prediction modes with directional angle delta
- 16 transform types across 19 sizes, all bit-exact with C SVT-AV1
- CDF-based entropy coding matching the rav1d decoder
- Speed presets 0-13 with progressive feature gating
- CQP, CRF, VBR, and CBR rate control
- Deblocking, CDEF, Wiener, and sgrproj loop filters
- Film grain estimation and synthesis parameters
- VAQ (variance-adaptive quantization) and perceptual QM

### What doesn't work yet

Some content/QP combinations fail to decode. Specifically: quality 70 with certain gradient patterns, some multi-SB sizes at certain speeds, and all-skip frames (uniform content). These are CDF context interaction bugs in the coefficient encoder that need a decoder-side trace comparison to diagnose. See [Known Bugs](#known-bugs) below.

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
