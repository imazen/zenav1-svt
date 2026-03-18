# SVT-AV1 Rust Port — Context Handoff

**Date:** 2026-03-17
**Tests:** 473 | **Warnings:** 0

## Status: Multi-SB decode works for PARTITION_NONE, fails for PARTITION_SPLIT

### What decodes successfully
- **All single-SB frames** (32x32, 48x48, 64x64): ALL quality levels (q30-q90), ALL speed presets (s4-s10)
- **Multi-SB with PARTITION_NONE** (speed 10): 128x128 at q50, 80x80 at s8
- **Deep partition search**: 64x64 at speed 4 (preset 4, depth 4) via zenavif

### What still fails
- Multi-SB frames when PARTITION_SPLIT is used at speed ≤8 (96x96+)
- Direct uniform gray multi-SB (all-skip frames fail even for single-SB)
- 128x128 gradient direct encode

### Fixes applied this session (8 commits, all pushed)

1. **Partition CDF update** — `write_partition` changed from `write_cdf` (no update) to `write_symbol` (with CDF update) to match decoder behavior.

2. **Complete OBU rewrite** — SH and FH rewritten from AV1 spec:
   - mono_chrome=1 (NumPlanes=1, no chroma syntax)
   - Fixed seq_choose_screen_content_tools (1-bit, not 2-bit)
   - Added render_and_frame_size_different, tile_info(), loop filter sharpness/delta
   - Removed CDEF bits, allow_intrabc, primary_ref_frame (all implicit)
   - tx_mode_select=0 (TX_MODE_LARGEST)

3. **Angle delta** — Added y_angle_delta encoding for directional modes.

4. **Mode context tracking** — EntropyCtx tracks above/left modes at 4x4 granularity for correct keyframe y_mode CDF context (was hardcoded (0,0)).

5. **Skip context tracking** — Actual neighbor skip status used for skip CDF context (was hardcoded 0).

6. **intra_mode_context fix** — Corrected mapping: D45-D67→2, SMOOTH→3, PAETH→4.

7. **SPLIT children** — Children always use (true, true) for partition context (within a split, siblings are never smaller).

8. **HORZ/VERT positions** — Correct spatial offsets for non-SPLIT partition children.

### Analysis of remaining failures

**Verified correct:**
- Range coder formula matches rav1d exactly: `EC_MIN_PROB * (n_symbols - val)` ≡ our `EC_MIN_PROB * (nsyms - 1 - s)` since rav1d's n_symbols = our nsyms - 1
- Default CDF values match AV1 spec (verified skip, partition CDFs)
- CDF update loop iterates same indices, update formula matches rav1d

**nsymbs mismatch (non-critical for current partition types):**
- Our nsyms for partition at ctx 12-15 (64x64) = 10, rav1d's n_symbols = 9
- The count field position differs (cdf[10] vs cdf[9]) but count VALUES are identical
- CDF update for indices 0-8 is identical. Only differs at index 9 (our ICDF sentinel vs decoder's count), which doesn't affect symbols 0-3 that we actually use

**Root cause of multi-SB PARTITION_SPLIT failures:**
The coefficient coding (`write_coefficients_ctx`) uses a completely different format from the AV1 spec:
- **EOB**: Our encoder writes as a variable-length literal. AV1 spec uses CDF-based multi-part coding (eob_pt_16/32/64, eob_extra).
- **Scan order**: Our encoder iterates 0..eob in raster order. AV1 spec uses a specific scan order (diagonal/zig-zag) and iterates in REVERSE for the base levels.
- **Level coding**: Our base context derivation (position + neighbor count) doesn't match the spec's SIG_COEF_CONTEXTS formula.
- **Sign coding**: We interleave signs with levels. The spec separates signs (coded after all levels).

For single-SB frames with one block, the decoder reads our "wrong" coefficient data and produces output (possibly wrong pixel values) without error because there's nothing after it. For multi-SB, the first SB's coefficient data consumes a different number of bits than expected, causing the decoder to read SB #2's partition symbol at the wrong position.

**Proof:** Frames that work with ALL blocks being skip (no coefficient data needed) still fail. This means the coefficient format mismatch is NOT the only issue — there's likely ALSO a frame header or tile structure issue for all-skip frames.

### Next steps

1. **Debug all-skip frames** — A single-SB all-skip frame (uniform gray 64x64) should be ~23 bytes and decode correctly. It has only 2 CDF symbols (partition + skip). If this fails, the issue is in the frame header, range coder finalization, or tile group structure.

2. **Hex-dump comparison** — Dump a passing bitstream (82 bytes) and a failing bitstream (23 bytes), parse them manually or with `ffprobe -show_packets`, to find exactly where the decoder rejects the data.

3. **Instrument rav1d** — Add tracing to rav1d's decode path to see which syntax element read fails.

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
- `crates/svtav1-entropy/src/context.rs` — All CDF contexts + EntropyCtx tracking
- `crates/svtav1-entropy/src/coeff.rs` — Coefficient entropy coding (needs rewrite)
- `crates/svtav1-entropy/src/range_coder.rs` — Arithmetic coder
- `crates/svtav1-encoder/src/pipeline.rs` — Block-level syntax writing + EntropyCtx
- `/home/lilith/work/zen/rav1d-safe/src/decode.rs` — Decoder reference
- `/home/lilith/work/zen/rav1d-safe/src/msac.rs:512` — Decoder range coder
