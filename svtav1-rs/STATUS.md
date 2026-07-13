# SVT-AV1 Rust Port — Status

Last updated: 2026-07-13 (wave2/entropy-c-parity) — C baseline **v4.2.0-rc**

## Decode conformance (AV1 reference decoder)

`tools/decode_conformance.sh` — 525-stream mono matrix (gradient/uniform/
edges x 32..128 px x qindex 30..90 x speeds 2..10) plus a 700-stream
4:2:0 matrix (`tools/decode_conformance.sh <dir> chroma`: same grid + a
`color` content whose chroma planes carry real patterns), every stream
must decode under **aomdec**:

| Gate | Result |
|---|---|
| 525/525 mono streams decode | **PASS** (was 0/525 before this wave) |
| 700/700 chroma-420 streams decode | **PASS** (new 2026-07-13; opt-in `with_chroma_420`) |

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
| Deblocking kernels (`svt_aom_lpf_{h,v}_{4,6,8,14}_c`) + sharpness limits | bit-exact over all (level, sharpness) x content classes (c_parity_lpf) |

v4.2.0-rc note: upstream refactored the coder internals (borrowed buffer,
ptr walk) — output verified still byte-identical; `coeff_br_cdf` dropped its
dead 64x64 slice (tables regenerated).

## Pixel-path status (decoded output correctness) — CORRECT

All probes decode via aomdec and compare against the source:

| Probe | Result |
|---|---|
| uniform-128, flat-140, flat-250 | **bit-exact** |
| edges 64px qindex30 s2 / 96px q50 s4 | **LOSSLESS** (205/367 bytes; C reference also lossless at 172 bytes — remaining delta is RD tuning) |
| gradient 64px qindex30 s4 | **46.76 dB** |
| gradient 128px q50 s8 | 30.39 dB |
| 420 probe 64px q30 (examples/probe_420) | Y 46.64 / U 52.97 / V lossless |
| 420 probe 128px q30 | Y 46.03 / U 51.92 / V 52.86 dB |
| 420 probe 128px q50 | Y 30.39 / U 55.98 / V 57.44 dB (Y == mono ref) |

Fixed en route: live-recon prediction neighbors, real mode/tx-type
signaling, AV1 quantizer tables + decoder-mirrored dequant, per-size
forward cos bits, restored inverse stage-range clamps, C-exact intra edge
fill (127/129/left[0]/above[0] rules), 64-dim coefficient zeroing.
Deblocking is now SIGNALED and applied decoder-exactly (2026-07-13): key
frames carry the q-picked loop_filter levels and the recon-parity gate
holds 216/0 with filtering live; CDEF/restoration stay disabled+unsignaled.
Per-SB QP offsets stay disabled until delta_q signaling is ported.

## Known failing test

(none — `multi_frame_bitstream_sizes_decrease` passes again since the
unsignaled loop filters were disabled: the filtered DPB recon had been
corrupting inter references, which was the real reason inter frames
outweighed the key frame. Workspace fully green.)

## Architecture direction

Module-by-module faithful port of C SVT-AV1 behind `svtav1-cref`
differential harnesses (see `docs/PORT-coeff-writer.md` for the worked
example). Bitstream writer layer (headers, tile groups, coefficient coding)
is now C-exact at the writer level; decision layers (partition/mode RDO,
filters, chroma) still ours and next in line:

1. Chroma 4:2:0 end-to-end — **landed 2026-07-13 (opt-in
   `with_chroma_420` + `encode_frame_420`; still-frame, UV_DC-only,
   min-8x8 luma policy — see CLAUDE.md gap 1a-1d for what remains
   toward C decision parity)**
2. Filter search + signaling ports — deblocking landed 2026-07-13
   (C-exact kernels + q-based level picker + decoder-exact frame walk,
   signaled in the FH; SSE-based level search and inter-frame levels still
   pending); CDEF/restoration next
3. Directional-mode edge extension (has_top_right/bottom_left)
4. Decision-layer parity vs C (partition/mode/TX RDO), then per-preset
   bitstream identity gates (see COVERAGE.md for the config-surface
   scoreboard: 121 fields auto-derived from EbSvtAv1EncConfiguration)

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
