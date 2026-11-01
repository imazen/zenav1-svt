# Changelog

## [Unreleased]

### QUEUED BREAKING CHANGES

- `StillInputFormat::Yuv444` adds a variant to the public input-format enum;
  downstream exhaustive matches must handle it.

### Added

- `AvifEncoder::encode_yuv444` routes 8-bit full-resolution color planes through
  the existing profile-1 still pipeline. Each plane accepts its own row stride.
  Coded-lossless preserves the supplied planes; RGB conversion is caller-owned.
  Depth, parity-policy and film-grain combinations outside the envelope refuse.
- The 4:4:4 reconstruction gate exercises the facade and raw pipeline together,
  and QP-0 cells additionally require exact source samples in independent decoders.

See [the implementation and verification record](../docs/AVIF-STILL-444.md).
