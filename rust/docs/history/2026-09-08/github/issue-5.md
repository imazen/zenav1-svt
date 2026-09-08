# Historical GitHub issue #5: QP 0 (CQP) still encodes emit valid-syntax bitstreams with garbage pixels; one 64x64 case rav1d-safe rejects

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **CLOSED**.
Original: https://github.com/imazen/zenav1-svt/issues/5. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

Found 2026-07-22 by the zenavif cross-backend RD sweep (zenavif `benchmarks/backend_sweep_2026-07-22.tsv`, rev pinned `3e25f52b5`).

**Symptom:** every CQP **QP 0** still-image encode in the sweep (115/115 cells: 12 photo images + mosaics, sizes 64-1024, presets ~1/7/12, 4:2:0 via `EncodePipeline::try_encode_frame_420`) produces a bitstream that:
- decodes without error on rav1d-safe AND zenav1-aom,
- **byte-identically on both** (so the bitstream is self-consistent),
- but to catastrophically wrong pixels: SSIMULACRA2 ≈ **−200 to −1100** vs the source (a QP 1 encode of the same input scores +80..+92).

So the corruption is encoder-side: QP 0 writes a valid-syntax stream encoding the wrong image. Boundary is exact: QP 1 clean, QP 0 corrupt (probed q97-q100 on 4 images — zenavif quality 99 → QP 1 fine, quality 100 → QP 0 corrupt).

Additionally one 64×64 QP-0 cell (CID22 image 1475938, 2 of 3 presets) produced a stream **rav1d-safe rejects outright** ("Failed to decode primary frame") — likely the same root cause surfacing as an invalid stream at single-SB size.

**Suspected area:** QP 0 implies lossless-mode signaling (`qindex==0` → coded-lossless path: WHT, deblock/CDEF/LRF forced off) — if the RD/recon side doesn't take the lossless transform path while the header signals it, decoders reconstruct garbage while the encoder believes its own recon. Consistent with CLAUDE.md's "lossless deprioritized" scope note: QP 0 is outside the verified envelope but `try_encode_frame_420` does not currently reject it.

**Ask:** either fix QP-0/lossless coding or (interim, per spec §5 no-silent-corruption) make `try_encode_frame*` return `EncodeError::UnsupportedConfig` for `qp == 0` until it's verified. zenavif's seam now clamps QP to ≥1 (`quality_to_qp_gated`, zenavif@svtav1-rs-backend) so quality 100 doesn't corrupt, but the library shouldn't hand any caller garbage at QP 0.

Repro (in-repo):
```rust
let rc = RcConfig { mode: RcMode::Cqp, qp: 0, ..Default::default() };
let mut p = EncodePipeline::new(512, 512, 7, rc, 0, 1).with_chroma_420(true);
p.bit_depth = 8;
let obu = p.try_encode_frame_420(&y, &u, &v, 512).unwrap();
// decode obu with aomdec/rav1d-safe -> planes massively diverge from recon/source
```
