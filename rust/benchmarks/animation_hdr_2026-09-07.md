# Animated ambient/content HDR metadata — 2026-09-07

The public SVT animation options now carry `AmveBox` and `CclvBox` into the
canonical animated serializer. Both properties are written to the color sample
entry, poster and uncropped secondary. Neither is copied to alpha. Raw AV1
coding and metadata-off output are unchanged by these additions.

The canonical parser previously discarded all four CLLI/MDCV/CCLV/AMVE boxes
in sample entries. `hdr-track-before.log` reproduces the lost CLLI value in a
valid sequence with its poster replaced by an equal-sized `free` box. The fixed
parser preserves track metadata independently of poster metadata through
`AnimationHdrMetadata` in parser, eager and native animation info.

## Wire and semantic evidence

- [AVIF 1.2 section 9](https://aomediacodec.github.io/av1-avif/v1.2.0.html#avif-items-and-properties)
  lists AMVE/CCLV as supported image properties without FullBox headers.
- [ITU-T H.274](https://www.itu.int/epublications/publication/itu-t-h-274-v3-2023-09-versatile-supplemental-enhancement-information-messages-for-coded-video-bitstreams),
  sections 8.13 and 8.14, specifies illuminance in 0.0001 lux, ambient xy in
  0..50000, signed content xy in -5000000..5000000, and normalized luminance
  increments of 0.0000001. CCLV requires a present field and ordered supplied
  luminances. `hdr-validation-before.log` records the missing validation before
  it was tightened in the shared box validator used by encoder and serializer.
- Libavif 1.3.0 `src/read.c::avifSkipContentColourVolume` and
  `avifSkipAmbientViewingEnvironment` confirm the BMFF field layout, including
  reserved zero cancel/persistence bits. This reader does not expose AMVE/CCLV
  values, so its decode success alone does not prove their preservation.

`animation_metadata_gate.py` additionally traverses box boundaries and checks
exact big-endian payloads, box paths, nonessential poster associations and
absence on alpha sample entries/items. It does not search compressed bytes for
box names. All 30,240 checks pass across 8/10-bit, color/monochrome,
straight/premultiplied/no alpha, repetition, aspect ratio, crop and orientation.
Libavif decodes every frame and checks the existing metadata and associations.

The serializer tests cover all 15 nonempty CCLV presence combinations, signed
boundaries, absent versus zero fields, invalid ranges/order, poster/track
independence and eager/owned/borrowed/reader paths. 89 tests pass. Parser
all-feature unit tests pass 20/20 plus 6 doctests. Seven actual managed-source
integration tests pass, including exact timing/count and metadata preservation
with unchanged native decoded pixels. Parser/serializer/managed-source lint
checks pass with warnings denied.

These canonical-source harnesses exclude unrelated unavailable optional/build/dev
sibling dependencies; this is not full canonical-workspace or all-feature
verification. They use the actual source files and normal required dependencies.
The local serializer dependency must still become a verified git pin before
landing. No CI or push was run.

Artifacts: `~/tmp/animation-metadata/hdr-*.log`, especially
`hdr-depth-metadata.log`, `hdr-serializer-final.log`, `hdr-native-final.log`,
`hdr-parser.log` and the clippy logs. Encoder final nextest (2600/2600) and regression (123/123) logs
are `hdr-nextest-final.log` and `hdr-spotcheck-final.log`. Workspace clippy
completed with existing warnings (`hdr-main-clippy-final.log`).
