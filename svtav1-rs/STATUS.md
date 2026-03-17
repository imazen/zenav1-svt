# SVT-AV1 Rust Port — Honest Status

**22,213 lines, 70 files, 451 tests, 51 golden parity tests**

## Genuinely Complete and Verified

### Transforms (spec 04) — 100% REAL
- 26/26 1D kernels: fdct/idct 4/8/16/32/64, fadst/iadst 4/8/16, fidentity/iidentity 4/8/16/32/64
- All **bit-exact with C SVT-AV1** (51 golden parity tests from measured C output)
- 19/19 TxSize 2D wrappers (square + rectangular, forward + inverse)
- 16/16 TxType dispatch through general framework
- SIMD dispatch on all functions (AVX2/NEON/scalar via archmage)

### Core Intra Prediction (spec 05) — ~80% REAL
- DC (3 variants), V, H, smooth/smooth_v/smooth_h, paeth — **real, tested**
- 8 directional modes (z1/z2/z3 zones) — **real algorithm from C source**
- Filter-intra (5 modes with tap table) — **real, ported from filterintra_c.c**
- CfL (luma subsampling + predict) — **real algorithm**
- Palette prediction — **real (trivial lookup)**
- IntraBC validity check — **real**; hash search — **simplified**

### Core Inter Prediction (spec 06) — ~60% REAL
- 8-tap sub-pixel convolution (horiz/vert/2D) — **real, with filter coefficients**
- Block copy, average, blend, distance-weighted blend — **real**
- OBMC blend masks — **real masks, but doesn't compute neighbor predictions**
- Warped motion — **nearest-neighbor only, not 8-tap interpolated**
- Scaled prediction — **nearest-neighbor only, not filtered**

### Entropy Coding (spec 07) — ~65% REAL
- Range coder (OdEcEnc) — **real, bit-exact CDF update**
- AomWriter — **real**
- OBU bitstream writer — **real for still-picture, simplified for inter**
- Coefficient coding structure — **real Exp-Golomb, but not using CDF tables**
- MV coding structure — **real class-based, but not using CDF tables**
- Tile coding — **real tile boundaries and multi-tile OBU format**
- FrameContext CDF shapes — **correct shapes, uniform init instead of AV1 defaults**

### Loop Filters (spec 08) — ~50% REAL
- Deblocking — **simplified 4-tap only** (spec has 4/6/8/14-tap + strength derivation)
- CDEF direction + filtering — **real algorithm**
- Wiener restoration — **real separable 7-tap filter**
- Self-guided restoration — **real box-filter + guided projection**
- Super-resolution upscale — **real 8-tap filter with AV1 coefficients**

### Encoder Core — ~50% REAL
- encode_block (predict→transform→quantize→reconstruct) — **real for 4x4/8x8/16x16/32x32**
- Partition search geometry — **all 10 types correct**
- Partition encoding — **only DC prediction at leaves, no mode decision**
- Mode decision candidates — **21 candidates listed but not evaluated in pipeline**
- ME (full-pel + half-pel) — **real search algorithm**

## Recently Wired Into Pipeline (previously orphaned)

- **CDEF** — now applied to reconstruction when `speed_config.enable_cdef` is true
- **Temporal filtering** — now called before encoding when refs available and enabled
- **Mode decision** — partition search now tries 5+ intra modes (DC/V/H/smooth/paeth)
- **Coefficient coding** — pipeline uses `write_coefficients` with Exp-Golomb, not literals
- **Speed config** — `enable_cdef`, `enable_temporal_filter`, `max_partition_depth` now honored

## Still Not Wired Into Pipeline

- **Film grain** — `estimate_film_grain()` + `synthesize_grain()` work, not called
- **Wiener/sgrproj restoration** — exist but pipeline only applies CDEF
- **Deblocking** — exists but not called in pipeline (only CDEF)
- **Multi-pass rate control** — `collect_first_pass_stats()` works, pipeline is single-pass
- **Remaining speed preset flags** — 17 of 20 flags still unused
- **MV coding** — real class-based coding exists, pipeline doesn't encode inter frames
- **Perceptual optimizations** — QM/VAQ/trellis exist, not used in encode path

## Not Implemented

- Full AV1 default CDF initialization tables (need ~400 lines of const data from spec)
- TPL (temporal propagation layer) for rate control
- Multi-threaded tile/segment parallelism
- Full inter-frame OBU headers (reference frame signaling, order hints)
- Proper deblocking strength derivation and 6/8/14-tap variants
- 8-tap warped motion interpolation (currently nearest-neighbor)
- 8-tap scaled prediction interpolation (currently nearest-neighbor)
- Full context derivation for all syntax elements
- AV1 bitstream conformance testing (decode our output with a reference decoder)

## TODO Priority Order

1. ~~Wire CDEF into pipeline~~ — DONE
2. ~~Wire mode decision into partition search~~ — DONE (5 intra modes)
3. ~~Wire temporal filter~~ — DONE (when refs available)
4. ~~Wire coefficient coding~~ — DONE (Exp-Golomb)
5. **Wire deblocking + Wiener/sgrproj into pipeline**
6. **Wire film grain** — estimate before encode, signal in bitstream
7. **Fix warped motion** — use 8-tap sub-pixel interpolation instead of nearest-neighbor
8. **Fix scaled prediction** — use filtered interpolation
9. **Fix deblocking** — add 6/8/14-tap variants and strength derivation
9. **Use speed config flags** — gate features based on preset
10. **Add default CDF tables** — initialize FrameContext from AV1 spec tables
11. **Wire multi-pass RC** — collect first-pass stats, use in second pass
12. **Add threading** — tile-parallel encoding
13. **Full inter OBU** — reference frame signaling for non-key frames
14. **Conformance testing** — decode our output with rav1d-safe or dav1d
