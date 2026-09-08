# Historical GitHub issue #6: Add a public u16 10-bit-source encode entry point

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/6. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

### Problem

The internal pipeline processes at **true 10 bits** and the bd10 gates byte-match real C (`bd10_photo` 191/191, `bd10_nonflat` 309/309, `bd10_matrix` 36/36). But the **public encode API is 8-bit-input only**:

```rust
EncodePipeline::encode_frame(&mut self, y: &[u8], stride)
EncodePipeline::try_encode_frame_420(&mut self, y: &[u8], u: &[u8], v: &[u8], stride)
```

`with_bit_depth(10)` sets the encode depth, but the exposed ingestion model is `src10 = src8 << (bd - 8)` (`leaf_funnel.rs:2095`) — so a consumer **cannot feed real 10-bit source**. The u16 source buffers (`y_src10: Vec<u16>`, `leaf_funnel.rs:2098`) live inside the encoder; the bd10 gates reach the true-10-bit path via the test-only `identity_run`, not the public API.

### Ask

Add public 10-bit-source entry points that ingest real `&[u16]` pixels into the existing true-10-bit path:

- `try_encode_frame_420_hbd(&mut self, y: &[u16], u: &[u16], v: &[u16], y_stride) -> EncodeResult<Vec<u8>>`
- `try_encode_frame_hbd(&mut self, y: &[u16], y_stride) -> EncodeResult<Vec<u8>>` (monochrome)

### Gate

Extend the bd10 gates to drive the **new public entry point** (not just the internal `identity_run`), so the public 10-bit surface is byte-identity-covered.

### Unblocks

zenavif exposing real 10-bit AVIF via the `Av1Backend::SvtRs` backend (currently `svt_rs_rejects_16bit_entry_points` is correct precisely because no u16 public API exists).
