# Animated AVIF and subsequent video completion

User objective (2026-09-07): **Get animated AVIF encoding working, with all
metadata, transparency, and spec supported features. After that, complete
video encoding support.** This objective remains active. The initial all-intra
path is an implementation step, not the completion boundary.

## Authoritative baseline

AVIF 1.2.0: https://aomediacodec.github.io/av1-avif/v1.2.0.html
AV1 ISOBMFF binding: https://aomediacodec.github.io/av1-isobmff/

Before this work, `AvifEncoder` returned raw still AV1 OBUs despite the crate
introduction claiming complete AVIF output. There was no animation API. The
root `Encoder::send_frame` discards its input and `receive_packet` always
returns NotReady. Do not treat that scaffold as video support.

## Implemented first step

* `EncodePipeline::with_image_sequence` emits full sequence/frame headers
  independently of all-intra coding policy. Still-picture flags are not used
  for animated samples.
* Optional `avif-container` feature uses zenavif-serialize 0.1.4. The serializer
  requires Rust 1.93; the default raw-AV1 feature set retains the manifest's
  Rust 1.89 minimum. No 1.89 toolchain is installed on this host, so the older
  toolchain has not been re-tested here.
* `AvifEncoder::encode_animation_yuv420` encodes each frame as a sync sample,
  with variable positive durations and a positive timescale, and synchronized
  optional monochrome alpha. Validates all input buffers before encoding.
  Alpha is full range and does not inherit color film grain.
* The codec level uses the fastest frame interval. Sequence headers and
  sample configuration share the same depth/profile/level derivation.
* `animation_probe` produces a real three-frame AVIF. Libavif 1.3.0 / dav1d
  1.5.3 decodes all frames, reports 100/200/300 ticks at timescale 1000, and
  recognizes alpha. Tests cover 64x64 and odd 65x67; every decoded PNG alpha
  sample equals a separately reconstructed monochrome encode.

## Required work before completion

Every row needs production wiring plus independent decoder/container evidence;
unsupported options and tests expecting refusal do not satisfy the goal.

| Requirement | Current state / next evidence |
|---|---|
| Frame timing, duration, count, seeking | Initial all-sync variable-duration path verified; broader boundaries, long durations, ordering and random access still needed |
| Finite/infinite repetition | Not implemented in wrapper; serializer's loop_count is stored but not consumed by serialize. Must implement edit-list/repetition signaling and verify reader behavior |
| Alpha | Initial 8-bit straight-alpha path verified. Lossless coverage, 10/12-bit, premultiplied association, opaque/missing-alpha handling and poster alpha remain |
| Metadata | CICP present in AV1 headers. ICC, Exif, XMP, CLLI/MDCV, metadata precedence and track/item associations not yet wired. Serializer animation API lacks ICC/Exif/XMP setters |
| Spatial properties | Clean aperture, rotation, mirror, pixel aspect, sequence track transformations and alpha alignment remain |
| Format coverage | u8 4:2:0 initial API only. Native 10-bit, 12-bit, 4:4:4/4:2:2, monochrome animation and lossless remain. C's rejection of some formats does not waive this broader user objective |
| AVIF specification features | Audit item/track brands and configuration, poster/primary item, auxiliary/depth tracks, collections, grids, layered/progressive items, gain maps/tone maps, sample transforms and entity groups against the full requested scope |
| Inter-picture compression | Still gated in the pipeline; all-sync animation does not close this requirement |
| Robust API | Streaming/bounded memory, cancellation, fallible allocation, overflow checks and complete validation remain to be audited across encoder and serializer |
| Independent conformance | Initial libavif/dav1d decode and alpha checks. Add rav1d-safe, container metadata round trips and broader sample-level recon comparisons |
| Video after animation | Replace root API scaffold; finish INTER reference/MV state, arbitrary-length GOPs, B frames/reordering, rate control, temporal filtering, all C-supported configuration and metadata wiring. Existing INTER-ENCODE-PLAN.md retains detailed evidence |

Artifacts on i265: `~/tmp/animation-probe.avif`, `animation-probe.log`,
`animation-decode.log`, `animation-tests.log`. These are initial evidence, not
an all-features completion claim. Preserve unrelated `nnlxmrsn` performance WIP.

Initial checkpoint verification: **2,582/2,582 workspace tests passed**, zero
skipped, with `--features zenav1-svt/avif-container`; **106/106** existing
regression spot-checks passed. The final animation tests additionally cover
color-only output and compare every decoded Y/U/V sample against independent
encoder reconstruction, as well as every alpha sample. Raw temporal delimiters
are omitted from container samples per AV1-ISOBMFF section 2.4. Final focused
results: `~/tmp/animation-tests-final.log`; broader logs:
`~/tmp/animation-nextest.log` and `~/tmp/animation-spotcheck.log`.
