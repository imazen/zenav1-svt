# SVT-AV1 Rust Port — Status

Last updated: 2026-07-13 (wave2/entropy-c-parity) — C baseline **v4.2.0-rc**

## Decode conformance (AV1 reference decoder)

`tools/decode_conformance.sh` — 525-stream matrix (gradient/uniform/edges x
32..128 px x qindex 30..90 x speeds 2..10), every stream must decode under
**aomdec**:

| Gate | Result |
|---|---|
| 525/525 streams decode | **PASS** (was 0/525 before this wave) |

The old rav1d-based "decode PASS" claims were leniency artifacts; aomdec is
the gate now.

## Bit-exact-vs-C differential suites (svtav1-cref harness)

All verified against the linked `libSvtAv1Enc.a` (v4.2.0-rc) on every test
run:

| Module | Verification |
|---|---|
| Range coder (`OdEcEnc`) | byte-identical streams: 30k update_cdf cases, 300 random static/adaptive streams, carry torture, tiny streams |
| `update_cdf` | bit-identical, all alphabet sizes 2–16 |
| Default CDF tables (13 coef families x 4 q-buckets + 12 mode families) | drift test re-extracts from C every run |
| Scan orders (19 x 3) | drift test vs `eb_av1_scan_orders` |
| Quantizer step tables | generated from `svt_aom_dc/ac_quant_qtx` |
| Coefficient writer helpers (level maps, nz/br ctx, eob tokens, txb dims) | fuzzed vs exported C impls |

v4.2.0-rc note: upstream refactored the coder internals (borrowed buffer,
ptr walk) — output verified still byte-identical; `coeff_br_cdf` dropped its
dead 64x64 slice (tables regenerated).

## Pixel-path status (decoded output correctness)

| Probe | Result |
|---|---|
| uniform-128 (all-skip) | decodes exactly (all 128) |
| flat-140 first block | decodes exactly 140 — fwd transform -> quant -> decoder dequant -> inverse chain is scale-correct |
| flat-140 full frame | ramps to 255: **encoder intra prediction ignores reconstructed neighbors** (predicts 128 fallback per block; decoder uses real neighbors; residuals compound). THE current pixel-path blocker. |
| gradient qindex 30 | 11.3 dB (garbage until the prediction plumbing is fixed) |

Also pending beneath it: unsignaled loop filters (encoder filters its recon,
headers say off) and per-SB QP offsets (now disabled until delta_q signaling
is ported).

## Known failing test

`multi_frame_bitstream_sizes_decrease` (real_encode): asserts key frame >
first inter frame in bytes. The corrected quantizer legitimately shrinks the
key frame (mostly-skip static content) while the primitive inter path spends
~230 bytes/frame on MV/mode overhead. Expectation left untouched pending a
decision (test-relaxation rule); it currently documents that the inter path
is unfinished.

## Architecture direction

Module-by-module faithful port of C SVT-AV1 behind `svtav1-cref`
differential harnesses (see `docs/PORT-coeff-writer.md` for the worked
example). Bitstream writer layer (headers, tile groups, coefficient coding)
is now C-exact at the writer level; decision layers (partition/mode RDO,
filters, chroma) still ours and next in line:

1. Encoder prediction from reconstructed neighbors (pixel-path blocker)
2. Chroma 4:2:0 end-to-end (C cannot emit monochrome — required for parity)
3. Filter search + signaling ports (deblock/CDEF/restoration)
4. Decision-layer parity vs C (partition/mode/TX RDO), then per-preset
   bitstream identity gates

## Crate structure

```
svtav1-rs/
  crates/svtav1-types          Core AV1 types, enums, constants
  crates/svtav1-tables         Const lookup tables (no_std)
  crates/svtav1-dsp            Transforms, prediction, filters, quant (+ generated quant tables)
  crates/svtav1-entropy        Range coder, CDFs, OBU, coefficient coding (+ generated CDF/scan tables)
  crates/svtav1-encoder        Pipeline, partition, mode decision, RC
  crates/svtav1-cref           Test-only FFI harness over libSvtAv1Enc.a (the differential oracle)
  crates/svtav1-disjoint-mut   Region-based borrow tracking
  svtav1                       Public API, AVIF backend
```

C reference builds required for tests:
`cmake -S . -B cbuild-static -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_APPS=OFF -DBUILD_TESTING=OFF && cmake --build cbuild-static -j`
