# SVT-AV1 Rust Port — Status

**26,095 lines | 70+ files | 500+ tests | 0 warnings**

Last updated: 2026-03-18

## Decode Conformance

Tested by encoding with svtav1-rs and decoding with rav1d-safe via zenavif differential tests.

| Test case | Result | Notes |
|-----------|--------|-------|
| 32x32 gradient (padded to 64x64) | PASS | Single SB, q50 s10 |
| 48x48 gradient (padded to 64x64) | PASS | Single SB, q50 s10 |
| 64x64 gradient, q30 | PASS | Single SB |
| 64x64 gradient, q50 | PASS | Single SB |
| 64x64 gradient, q60 | PASS | Single SB |
| 64x64 gradient, q90 | PASS | Single SB |
| 128x128 edges, q80 | PASS | 4 SBs, PSNR 11.1 dB |
| 128x128 gradient (direct) | PASS | 4 SBs |
| comprehensive_all_configs (48) | PASS | All size/quality/speed combos |
| 64x64 gradient, q70 | FAIL | Content-specific CDF interaction |
| 80x80, 96x96, 112x112 | FAIL | Some multi-SB combos at certain speeds |
| 64x64 speed sweep (s4-s8) | FAIL | q70 content-specific |
| Uniform 128x128 (all-skip) | FAIL | All coefficients zero |
| Direct 64x64 (all-skip) | FAIL | All coefficients zero |

## Verified Bit-Exact With C (51 golden parity tests)

Every value verified by compiling the C SVT-AV1 function via `tools/extract_golden.c` and comparing Rust output coefficient-by-coefficient.

| Function | Vectors | C Source |
|----------|---------|----------|
| fdct4/8/16/32/64 | 16 | transforms.c:svt_av1_fdct{N}_new |
| fadst4/8/16 | 6 | transforms.c:svt_av1_fadst{N}_new |
| fidentity4/8 | 2 | transforms.c:svt_av1_fidentity{N}_c |
| idct4/8/16/32/64 | 10 | inv_transforms.c:svt_av1_idct{N}_new |
| iadst4/8/16 | 4 | inv_transforms.c:svt_av1_iadst{N}_new |
| iidentity4/8 | 2 | inv_transforms.c:svt_av1_iidentity{N}_c |
| cospi/sinpi tables | 10 | inv_transforms.c:svt_aom_eb_av1_cospi_arr_data |
| CDF update | 2 | Identical C algorithm |
| Intra prediction | 5 | Spec algorithm (DC/V/H/paeth verified) |
| Roundtrip DCT 4/8/16/32 | 4 | Scale factor verification |

## Pipeline Architecture

```
encode_frame(y_plane)
  temporal_filter (if enabled + refs available)
  ActivityMap::compute -> VAQ QP adjustment
  TPL QP adjustment (inter frames)
  for each 64x64 superblock (raster order):
    partition_search_with_config
      tries 10 partition types (gated by speed config)
      encode_single_block at each leaf:
        11 intra modes with TX-type RDO
        encode_block_tx -> transform -> quantize -> reconstruct
  deblock_vert/horz (4/8/14-tap, per 8x8 edges)
  CDEF (per 8x8 block, directional)
  Wiener restoration
  sgrproj restoration (preset <= 6)
  film_grain estimation
  entropy coding:
    partition context tracking (rav1d-compatible AL_PART_CTX)
    write_coefficients_v2 (CDF-based EOB, reverse scan, spec tokens)
  OBU output (TD + SH + Frame)
  DPB refresh + RC state update
```

## Entropy Coding (write_coefficients_v2)

Spec-conformant implementation matching rav1d's exact bitstream reading order:

1. TXB skip flag (CDF, 2 symbols)
2. TX type (CDF, 5 or 7 symbols; implicit for blocks >= 32x32)
3. EOB bin (CDF, 5-11 symbols depending on TX size class)
4. EOB hi-bit + equiprobable low bits
5. EOB position token (eob_base_tok, 3-symbol CDF)
6. AC tokens in reverse diagonal scan (base_tok, 4-symbol CDF)
7. BR tokens for levels >= 3 (br_tok, 4-symbol CDF, up to 4 iterations)
8. DC token (base_tok or eob_base_tok)
9. DC sign (CDF, 2 symbols)
10. DC Golomb residual (if token >= 15)
11. AC signs (equiprobable) + Golomb residuals

Default CDFs extracted from rav1d for 4 QP categories (qidx 0-20, 21-60, 61-120, 121+). Context derivation uses `get_lo_ctx_2d` with `LO_CTX_OFFSETS` lookup table matching `dav1d_lo_ctx_offsets`.

## Partition Context Tracking

Multi-SB conformance uses rav1d-compatible partition context arrays:

- `above_partition` / `left_partition` at 8x8 granularity
- `AL_PART_CTX` lookup table matching `dav1d_al_part_ctx` (5 block levels x 10 partition types x 2 directions)
- Bit extraction: `(val >> bsl) & 1` for context derivation
- Left context reset per SB row

## Speed Config — Feature Gating

| Flag | Wired | Presets |
|------|-------|---------|
| max_partition_depth | Yes | 4 (p0-3), 3 (p4-6), 2 (p7-9), 1 (p10-13) |
| enable_cdef | Yes | p0-12 |
| enable_restoration | Yes | p0-10 |
| enable_temporal_filter | Yes | p0-12 |
| lambda_scale | Yes | 1.0 (p0-3), 1.1 (p4-6), 1.2 (p7-9), 1.4 (p10-13) |
| enable_ext_partitions | Yes | p0-8 (T-shapes: HorzA/B, VertA/B) |
| enable_4to1_partitions | Yes | p0-6 (Horz4, Vert4) |
| enable_directional_modes | Yes | p0-10 |
| enable_adst | No | Always tried when mode matches |
| rdo_tx_decision | No | TX RDO always runs |
| max_intra_candidates | No | Not passed to encode_single_block |
| Inter features (ME, compound, warped) | No | Inter infrastructure present but not wired |

## Crate Structure

```
svtav1-rs/
  svtav1-types/          2,016 lines  Core AV1 types, enums, constants
  svtav1-tables/           350 lines  Const lookup tables (no_std)
  svtav1-dsp/           10,700 lines  SIMD transforms, prediction, filtering
  svtav1-entropy/        5,661 lines  Range coder, CDF, OBU, coefficient coding
  svtav1-encoder/        6,093 lines  Pipeline, partition, mode decision, RC
  svtav1-disjoint-mut/     226 lines  Region-based borrow tracking
  svtav1-cuda/               4 lines  GPU bridge stub
  svtav1/                1,045 lines  Public API, AVIF backend
```

## Measured Performance

(Release mode, x86_64 AVX2, archmage auto-vectorization)

| Operation | Throughput |
|-----------|-----------|
| SAD 16x16 | ~18 Gpix/s |
| fwd_txfm 4x4 | ~170 Mpix/s |
| fwd_txfm 8x8 | ~215 Mpix/s |

## Test Inventory

| Category | Count |
|----------|-------|
| Golden parity (bit-exact C comparison) | 51 |
| Spec-referenced (AV1 section citations) | 20 |
| E2E correctness | 44 |
| Real encoding (actual images, PSNR) | 15+ |
| SIMD dispatch (for_each_token_permutation) | 24 |
| Coefficient encoder v2 | 16 |
| Unit tests | ~330 |
| **Total** | **500+** |
