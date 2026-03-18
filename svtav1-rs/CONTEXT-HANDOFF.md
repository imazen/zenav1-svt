# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-17
**Tests:** 470 | **Warnings:** 0

## BLOCKING: Multi-SB decode failure

Single-SB frames (up to 64x64 padded) decode with rav1d-safe at ALL quality levels and ALL speed presets. Multi-SB frames (128x128+ = 2+ SBs at sb_size=64) fail with "AV1 decode error -1". This is the ONLY remaining conformance issue.

### What works
- 64x64 gradient: DECODES at q30/q60/q70/q90, speeds 4-10
- 32x32 padded to 64x64: DECODES
- OBU structure verified correct by both rav1d-safe and ffprobe (after trailing_one_bit fix)
- Frame data for the first SB is byte-identical between 64x64 standalone and 128x128 multi-SB

### What fails
- 128x128 gradient (2×2 = 4 SBs): FAIL
- 128x64 gradient (2×1 = 2 SBs): FAIL
- 80x80 (padded to 128x128, 4 SBs): FAIL
- ANY frame with >1 SB fails

### Root cause investigation so far

1. **CDF format** — FIXED. CDFs were stored in CDF format but encoder expects ICDF. Converted all 9 default tables. (6a611ba6)
2. **CDF update logic** — FIXED. update_cdf now matches rav1d's msac.rs exactly for ICDF values. (b813e500)
3. **sb_size mismatch** — FIXED. SH signals use_128x128_superblock=0 → sb_size=64. Encoder was using 32. (cb349569)
4. **trailing_one_bit** — FIXED. Always written per spec, even when byte-aligned. (9852ac19)
5. **Partition context** — FIXED. Position-aware has_above/has_left for SB boundaries. (0025d962)
6. **Range coder underflow** — FIXED. Clamped range to minimum 1. (568d1a4a)
7. **CoeffContext CDFs** — FIXED. base_cdf and br_cdf initialized with proper uniform distributions. (6bbbcdec)

### Key observation
The first SB's encoded data in a 128x128 bitstream is byte-identical to a standalone 64x64 bitstream. The decoder successfully parses SB #1 but fails when reading SB #2. This means the encoder's state after SB #1 (CDF tables, range coder state) diverges from the decoder's state.

### Most likely remaining causes (in priority order)

1. **Coefficient coding syntax mismatch** — Our coefficient encoding (write_coefficients_ctx) may not match what the decoder reads. The decoder reads coefficients in a specific scan order with specific context derivation. Our base_ctx derivation (position + neighbor count) may differ from the spec's exact SIG_COEF_CONTEXTS formula. After one SB of coefficient coding, the CoeffContext CDFs diverge.

2. **write_symbol vs write_cdf inconsistency** — Some syntax elements use write_symbol (with CDF update) and some use write_cdf (without update). The decoder always updates CDFs. If we mix updated and non-updated CDFs, the states diverge. Currently write_partition uses write_cdf (no update) while skip/mode use write_symbol (with update). The decoder updates ALL CDFs.

3. **Intra mode CDF mismatch** — Our kf_y_mode_cdf has 13 symbols. The write_intra_mode_kf passes INTRA_MODES=13 as nsymbs. The CDF array has 14 entries (13 + sentinel). If the ICDF sentinel or count position is wrong, the CDF update corrupts adjacent memory.

4. **Missing syntax elements** — The AV1 spec may require additional syntax elements per block that we don't encode (e.g., tx_size, tx_type as separate syntax, filter type). Each missing element causes the decoder to read bits from the coefficient data, misaligning everything.

### Files to investigate

- `crates/svtav1-entropy/src/coeff.rs` — write_coefficients_ctx, base_ctx derivation
- `crates/svtav1-entropy/src/cdf.rs` — update_cdf (already matched to rav1d)
- `crates/svtav1-entropy/src/context.rs` — write_partition, write_skip, write_intra_mode_kf
- `crates/svtav1-encoder/src/pipeline.rs` — encode_partition_tree, tile data assembly
- `/home/lilith/work/zen/rav1d-safe/src/msac.rs` — decoder's CDF update (reference)
- `/home/lilith/work/zen/rav1d-safe/src/decode.rs` — decoder's syntax reading (reference)

### Debugging approach

1. **Instrument rav1d-safe** to print each syntax element it reads (symbol, context, CDF state) for a 128x128 frame
2. **Instrument our encoder** to print each syntax element it writes
3. **Compare the two traces** — the first divergence point is the bug
4. Alternative: encode a 128x128 frame with `disable_cdf_update=1` in the frame header AND disable updates in the encoder. If this decodes, the issue is CDF update divergence. If it still fails, the issue is in the base syntax.

### Quick test commands

```bash
# Run from svtav1-rs/
cd /home/lilith/research/svtav1/svtav1-rs
cargo test --workspace  # 470 tests, all pass

# Run decode tests from zenavif/
cd /home/lilith/work/zen/zenavif
cargo test --features "encode,encode-svtav1" --test differential_svtav1 -- "svtav1_decode" --nocapture

# Key tests:
# svtav1_decode_direct_gradient_64x64  — PASSES (single SB)
# svtav1_decode_direct_gradient_128x128 — FAILS (4 SBs)
# svtav1_decode_roundtrip_gradient — PASSES (64x64 via zenavif)
```

### Build
```bash
cd /home/lilith/research/svtav1/svtav1-rs
cargo test --workspace          # 470 tests
cargo clippy --workspace --all-targets  # 0 warnings
```

### Rules
- **CONFORMANCE MANDATE**: NEVER stop working while decode failures remain
- `#![forbid(unsafe_code)]` on all crates
- NEVER fabricate performance numbers
- Report gaps FIRST, progress second
