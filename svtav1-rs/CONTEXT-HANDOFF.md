# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-17
**Tests:** 473 | **Warnings:** 0

## Status: Multi-SB decode partially working

Single-SB frames (up to 64x64) decode correctly at ALL quality levels and ALL speed presets with rav1d-safe. Multi-SB frames decode for SOME configurations (speed 10, certain sizes at speed 8) but fail for others.

### What works
- **All single-SB frames**: 32x32, 48x48, 64x64 — all quality levels (q30-q90), all speeds (s4-s10)
- **Some multi-SB frames**: 80x80 at s8 (2×2 SBs), 128x128 at s10 q50 (via zenavif)
- OBU structure validated by ffprobe and rav1d-safe
- Monochrome (NumPlanes=1) encoding — no chroma syntax in bitstream

### What fails
- 96x96, 112x112, 128x128 at speed 8 (via zenavif) — multi-SB
- 80x80 at speed 10 — multi-SB
- Direct 128x128 (gradient and uniform) — multi-SB
- Most multi-SB configurations at speeds where partition splitting occurs

### Fixes applied this session

1. **Partition CDF update** (9c9d45bd) — `write_partition` was using `write_cdf` (no CDF update) while the decoder always updates. Changed to `write_symbol`.

2. **OBU rewrite** (03081990) — Complete rewrite of SH and FH to match AV1 spec:
   - SH: switched to `mono_chrome=1` (NumPlanes=1) — eliminates chroma delta-Q, uv_mode, chroma coefficients
   - SH: fixed `seq_choose_screen_content_tools` from 2-bit to 1-bit
   - FH: added `render_and_frame_size_different`, `tile_info()`, loop filter sharpness/delta
   - FH: removed CDEF bits (enable_cdef=0), allow_intrabc (implicit), primary_ref_frame (implicit for error_resilient KEY_FRAME)
   - FH: set `tx_mode_select=0` (TX_MODE_LARGEST) — no per-block tx_size needed

3. **Angle delta** (03081990) — Added `y_angle_delta` encoding for directional intra modes (V_PRED through D67_PRED). The decoder reads this after y_mode.

4. **Partition context revert** (c1535dc4) — The `has_above/has_left` based partition context produces decodable bitstreams for single-SB. The exact context derivation needs investigation to match rav1d's mi-grid based computation for multi-SB.

### Root cause analysis for remaining failures

The remaining multi-SB failures occur specifically when the encoder uses **PARTITION_SPLIT** (speed 8 and below). When all SBs use PARTITION_NONE (speed 10+), multi-SB frames can decode.

**Most likely causes:**

1. **Partition context for sub-blocks within SPLIT** — Within a PARTITION_SPLIT, the 4 child blocks need partition contexts based on the decoder's mi-grid tracking. Our encoder passes `(true, true)` for all children, but the actual contexts depend on which children have already been decoded and their sizes.

2. **Coefficient coding format mismatch** — Our `write_coefficients_ctx` uses simplified literal EOB and position-based contexts that may not match the AV1 spec's multi-part CDF-based EOB coding (eob_pt_16/32/64/etc). The coefficient scan order and context derivation also need verification.

3. **Partition context bsl/nsymbs mismatch** — Our bsl mapping (0=8x8, 3=64x64) and nsymbs (4/10/8) may differ from what rav1d uses internally. Investigation needed to verify the flattened CDF indexing matches rav1d's 2D `partition[BlockLevel][sub_ctx]` layout. rav1d uses symbol counts [7, 9, 9, 9, 3] indexed by BlockLevel (0=128x128, 4=8x8).

### Investigation approach

1. **Instrument encoder** to log each syntax element written per SB (symbol, context, CDF state)
2. **Instrument rav1d** to log each syntax element read per SB
3. **Compare traces** for a failing multi-SB frame — first divergence is the bug
4. Alternative: encode with `PARTITION_NONE` forced for all SBs regardless of speed preset. If this makes multi-SB decode work at s8, the issue is confirmed in PARTITION_SPLIT handling.

### Quick test commands

```bash
cd /home/lilith/research/svtav1/svtav1-rs
cargo test --workspace          # 473 tests, all pass
cargo clippy --workspace --all-targets  # 0 warnings

cd /home/lilith/work/zen/zenavif
cargo test --features "encode,encode-svtav1" --test differential_svtav1 -- "svtav1_decode" --nocapture
```

### Key files
- `crates/svtav1-entropy/src/obu.rs` — Sequence header, frame header, tile info
- `crates/svtav1-entropy/src/context.rs` — Partition, skip, mode, angle_delta CDFs
- `crates/svtav1-entropy/src/coeff.rs` — Coefficient entropy coding
- `crates/svtav1-encoder/src/pipeline.rs` — Block-level syntax writing order
- `/home/lilith/work/zen/rav1d-safe/src/decode.rs` — Decoder reference
- `/home/lilith/work/zen/rav1d-safe/src/env.rs:94` — Decoder partition context

### Build
```bash
cd /home/lilith/research/svtav1/svtav1-rs
cargo test --workspace          # 473 tests
cargo clippy --workspace --all-targets  # 0 warnings
```

### Rules
- **CONFORMANCE MANDATE**: NEVER stop working while decode failures remain
- `#![forbid(unsafe_code)]` on all crates
- NEVER fabricate performance numbers
- Report gaps FIRST, progress second
