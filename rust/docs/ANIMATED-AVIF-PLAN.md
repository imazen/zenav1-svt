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
* Optional `avif-container` feature uses zenavif-serialize 0.2.0 pinned to canonical git commit `7b058bb8`. The serializer
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
| Finite/infinite repetition | Writes edit lists and finite/infinite presentation durations; 18 libavif track/poster checks pass. Canonical parser derives finite play count from track/edit duration; serializer pinned to `7b058bb8` |
| Alpha | 8-bit straight/premultiplied associations and poster alpha verified, alongside color-only output. Lossless coverage, 10/12-bit and opaque/missing-alpha policy remain |
| Metadata | ICC/Exif/XMP/CICP/CLLI/MDCV wiring covers color track and poster. Libavif verifies exact ICC/Exif/XMP plus CICP/CLLI; independent box traversal verifies MDCV values/placement. Precedence and broader metadata audit remain |
| Spatial properties | Clean aperture, rotation, mirror, pixel aspect, sequence track transformations and alpha alignment remain |
| Format coverage | 8-bit and native 10-bit 4:2:0 APIs, including native alpha. 10-bit currently inherits pipeline alignment/preset restrictions. 12-bit, 4:4:4/4:2:2, monochrome animation and lossless remain. C's rejection of some formats does not waive this broader user objective |
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

Metadata continuation (2026-09-07): `AnimationOptions` carries
repetition, ICC, Exif, XMP, CLLI, MDCV and premultiplied-alpha association.
Poster alpha is now serialized. `tools/animation_metadata_gate.py` independently
checks 18 track/poster combinations using `tools/avif_metadata_probe.c` and
libavif 1.3.0. All passed, including exact metadata bytes. Serializer tests
78/78 and all 19 parser unit tests passed using source-symlink standalone
harnesses (the canonical workspace has an unavailable zenanalyze path dependency).
Evidence: `~/tmp/animation-metadata/{live-gate,repetition-tests,parser-repetition-tests,svt-final-tests}.log`.
The main manifest pins canonical serializer revision `7b058bb825f64a05ed97ac057178c80d27811853`; focused animation tests and all 18 metadata cases passed against the fetched git source. The CI job is preserved in jj change `puzrqvms`. Its pinned libavif 1.3.0 build recipe and all checks passed locally before attempting to land it, as requested. GitHub still refuses the workflow because the active gh token has repo/read:org/gist scopes but no workflow scope. Rebase that change onto main and push after `gh auth refresh -h github.com -s workflow`. Full scope above remains open.

Native 10-bit continuation: `AnimationFrame<T = u8>` accepts
`u16` through `encode_animation_yuv420_hbd[_with_options]`. Both color and alpha
use native pipeline entry points and matching high-bit-depth `av1C` properties.
A decode test checks all YUV samples against 10-bit reconstruction, then recovers
native alpha samples from libavif's 16-bit PNG output and compares them exactly.
It covers qualities 40/98, two frames, variable duration, strided luma, and
nonzero low two bits. This exposed missing monochrome 10-bit post-filter recon:
the canvas required chroma planes and its search/apply calls hardcoded 4:2:0.
The fix carries the monochrome canvas through those same filters.
Remaining native restrictions (64-aligned dimensions and preset >=9 for alpha)
are capability gaps to close, not completion claims. Evidence:
`~/tmp/animation-metadata/hbd-tests.log`. Final local verification: 2,584/2,584 workspace nextest tests, 106/106 regression cases, and 18/18 metadata cases with a locally built libavif 1.3.0 using the proposed CI recipe. Logs: `hbd-nextest-final.log`, `hbd-spotcheck.log`, `ci-recipe-metadata.log` in the same directory. Clippy completed with existing encoder warnings; changed Rust files pass rustfmt checks.
