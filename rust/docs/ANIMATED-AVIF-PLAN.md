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
| Frame timing, duration, count, seeking | All-sync timing verified through u32-max sample durations, totals above 32 bits and 257 frames; reverse/alternating seeks and reset replay preserve all color/alpha bytes. Full finite-count and exact downstream timing APIs remain |
| Finite/infinite repetition | Writes edit lists and finite/infinite presentation durations; 18 libavif track/poster checks pass. Canonical parser derives finite play count from track/edit duration; serializer pinned to `7b058bb8` |
| Alpha | 8-bit straight/premultiplied associations and poster alpha verified, alongside color-only output. Lossless coverage, 10/12-bit and opaque/missing-alpha policy remain |
| Metadata | ICC/Exif/XMP/CICP/CLLI/MDCV wiring covers color track and poster. Libavif verifies exact ICC/Exif/XMP plus CICP/CLLI; independent box traversal verifies MDCV values/placement. Precedence and broader metadata audit remain |
| Spatial properties | Square-pixel aspect, clean aperture, rotation and mirror metadata wired to color track/poster; uncropped secondary shares encoded samples and retains alpha/metadata. Displayed transformed-alpha pixel verification remains |
| Format coverage | 8-bit and native 10-bit 4:2:0 APIs, including native alpha. Native alpha currently requires preset >=9. 12-bit, 4:4:4/4:2:2, monochrome animation and lossless remain. C's rejection of some formats does not waive this broader user objective |
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
Correction: the 64-alignment claim was stale; both native level producers
already supported partial superblocks. The remaining native alpha preset >=9
restriction is a capability gap to close, not a completion claim. Evidence:
`~/tmp/animation-metadata/hbd-tests.log`. Final local verification: 2,584/2,584 workspace nextest tests, 106/106 regression cases, and 18/18 metadata cases with a locally built libavif 1.3.0 using the proposed CI recipe. Logs: `hbd-nextest-final.log`, `hbd-spotcheck.log`, `ci-recipe-metadata.log` in the same directory. Clippy completed with existing encoder warnings; changed Rust files pass rustfmt checks.

Odd-size continuation (2026-09-07): native monochrome now pads the real u16
source and its u8 mode-decision input to the same internal dimensions. The
expanded animation tests exposed two separate reconstruction defects:

- An 8-bit cached chroma block copy crossed the destination row at a partial
  right edge. Clipping the destination while retaining the transform source
  stride fixes it. The 65x67 speed-2 (preset-1) fixture previously differed in 656
  unfiltered chroma bytes from dav1d.
- C's deblock search uses truncated odd chroma bounds. Output reconstruction
  now replays the signaled filters with ceiling bounds after encoding decisions
  are finished, preserving the C search policy. The native 65x65 fixture
  previously differed in 277 filtered bytes; unfiltered reconstruction was exact.

The three animation tests pass with varied chroma, odd dimensions, strided input,
8/10-bit color and alpha, and native reconstruction-output byte invariance.
Evidence: `~/tmp/animation-metadata/odd-full-tests.log`. The final workspace
run passes 2,586/2,586 tests with zero skips (`odd-nextest-final.log`). Two
standalone aomdec tests reproduce the animation witnesses; removing the fixes
makes both fail at the original first differing bytes (`odd-repro-before.log`).
The regression spot-check now passes 108/108 with zero skips, including both
tests (`odd-spotcheck-final.log`). The full 8-bit C identity
sweep passes 1,100/1,100, with no pinned differences or harness errors
(`odd-identity-full.log`, `odd-identity-full.tsv`). Independent libavif metadata
verification passes 18/18 (`odd-metadata.log`) with a fresh release example.
Clippy passes with the existing 1,843 encoder warnings and no animation/new-test
warnings (`odd-clippy-final.log`); scoped rustfmt and refusal inventory checks
pass. Native monochrome mode decision still
uses the upper 8 bits, while coded levels and filters use the full 10 bits.

Superresolution continuation: the initial six odd-size cases passed, but a
216-case decoder grid (widths 65/66/72, heights 65/67/72, denominators 9/12/16,
presets 7/9, qualities 5/40/75/98) exposed the same chroma filter-bound defect
before upscaling. The 65x65 / denominator 9 / preset 7 / quality 5 witness first
differed at byte 5214. Output reconstruction now uses normative filter bounds
before the shared upscale stage, separately from the reference-picture canvas.
All 216 comparisons pass after the change (`odd-superres-after.log`); the
regression gate includes the witness grid. The identity driver's final-recon
dump now uses ceiling chroma stride at odd upscaled widths. Local workspace
nextest passes 2,587/2,587 with zero skips (`odd-superres-nextest.log`), the
expanded regression gate passes 109/109 (`odd-superres-spotcheck.log`), and the
superresolution C-byte/decode gate passes 512/512 (`odd-superres-identity.log`).
CI remains deferred while subsequent features are implemented and checked.

Pixel-aspect continuation: `AnimationOptions::pixel_aspect_ratio` and the
canonical serializer's animation setter write `pasp` on the color track and
poster. AVIF 1.2 section 9.1.2 requires a 1:1 ratio, so zero and non-square
ratios are rejected before encoding. Valid explicit values are preserved,
including 2:2 and u32::MAX:u32::MAX. Source: https://aomediacodec.github.io/av1-avif/v1.2.0.html#requirements-on-additional-image-item-related-boxes
All 79 serializer tests pass, including a canonical-parser round trip and
invalid-ratio checks. The independent libavif gate now passes 72/72 metadata
cases, and main workspace nextest passes 2,588/2,588 with zero skips. Evidence:
`~/tmp/animation-metadata/pasp-{serializer,metadata,nextest}.log`. The main
manifest temporarily uses the canonical sibling path while changes remain local;
replace it with the verified git revision when landing. CI remains deferred.

Rotation/mirror continuation: the facade and canonical animated serializer now
validate and write counter-clockwise quarter turns and HEIF mirror axes on the
color track and primary poster. Poster transforms are essential, ordered
rotation then mirror, and are not duplicated on associated alpha. All 15
optional rotation/mirror combinations round-trip through the canonical parser.
The independent libavif gate passes 1,080/1,080 cases across repetition, alpha,
pixel aspect, orientation and track/poster selection; structural checks verify
exact payloads, property ordering, essential bits and alpha associations.
Workspace nextest passes 2,589/2,589 with zero skips, serializer tests pass 80/80,
and the regression gate passes 109/109. Serializer clippy passes with warnings
denied; facade clippy completes with existing encoder/test warnings. Evidence:
`~/tmp/animation-metadata/rotation-{serializer,metadata,nextest,spotcheck,clippy}.log`.
These checks establish decoded metadata and associations; displayed transformed
pixel comparison remains separate work. The audit also corrected canonical
zenavif's reversed HEIF mirror-axis mapping, verified against all 12 combinations
from the actual libavif C helper (see its Known Bugs entry). Both repositories
remain local with the temporary sibling serializer dependency; CI stays deferred.

Clean-aperture continuation: `AnimationOptions::crop` uses `CropRect` with
integer coordinates before orientation, as required by MIAF. Validation rejects
empty, overflowing or out-of-bounds crops before encoding. The canonical
serializer writes exact rational center offsets in `clap`, then rotation and
mirror properties. Every cropped animation includes a non-hidden uncropped
secondary poster (item 5), meeting AVIF 1.2 section 2.2.3 for off-center crops.
The secondary color and optional alpha items share the first frame's encoded
extents; no additional frame is encoded. Descriptive properties and sidecars
also describe the uncropped image, which has no transformative properties.

Independent libavif verification initially exposed its single-target handling
of `auxl`/`cdsc`: a multi-target Exif reference lost metadata on the primary
poster (`crop-metadata.log`). Separate reference source items now retain alpha,
Exif and XMP on both posters. Item 6 shares the first alpha extent; sidecars
7/8 describe the secondary independently. The gate changes only `pitm` in a
copy to decode the actual secondary through libavif's primary-item API, and
checks shared extents, visibility, exact metadata and alpha/premultiplication.
All 7,560 combinations pass (`crop-metadata-final.log`) across four crops
(including odd origins/dimensions and a 1x1 corner), optional orientation, pixel
aspect, repetition and alpha. Serializer tests pass 82/82, workspace nextest
passes 2,590/2,590 with zero skips, and serializer clippy passes with warnings
denied; facade clippy completes with existing warnings. Evidence under
`~/tmp/animation-metadata/crop-{secondary-build,nextest,clippy}.log`. The
regression gate passes 109/109 (`crop-spotcheck.log`); scoped Rust formatting
and diff whitespace checks pass.
CI remains deferred and the temporary sibling serializer dependency remains
until the verified canonical commits can be landed and pinned.

Timing/random-access continuation: `tools/animation_timing_gate.py` verifies
27 cases with libavif, including run-length duration tables, one-frame output,
u32-max durations/timescales, total durations above u32, finite/infinite repeat,
and 257-frame sequences. Structural checks compare exact 64-bit movie/track
presentation durations, media duration, edit lists, sample timing and sync
indexes on color and alpha. The C probe compares every plane byte after
reverse seeks, alternating seeks and reset/replay against sequential decoding;
all fixture frames must be distinguishable to make wrong-index seeks visible.
All 27 cases and all 7,560 metadata cases pass on the final release example
(`~/tmp/animation-metadata/timing-independent-final.log`).

The canonical parser now exposes `frame_timing(index)` with exact ticks and
64-bit timestamps. Its previous public frame timing was whole milliseconds,
truncated and saturated, so it could not represent the encoded timing range.
83 serializer tests, 19 parser tests and 5 parser doctests pass; standalone
source harnesses both pass clippy with warnings denied. Main clippy completes
with existing warnings. Evidence: `timing-parser-serializer.log` and
`timing-clippy.log`. Managed/codec decoder adapters still use the legacy
millisecond field and require follow-up wiring. A separate verified gap remains:
canonical parser `loop_count: u32` rejects 4,294,967,296 finite playbacks, though
the serializer writes the correct presentation duration. Libavif deliberately
reports counts above INT_MAX as infinite; the gate independently checks exact
box values for those cases. See canonical Known Bugs for the reproducer.

Before eventually landing preserved workflow change `puzrqvms`, add the new
timing gate alongside its metadata gate. CI is still deferred; local serializer
path staging remains until the canonical changes can be landed and pinned.

Final timing checkpoint: all 2,590 workspace tests pass with zero skips and all
109 regression checks pass (`timing-local-final.log`). Scoped Rust formatting
and whitespace checks pass. This closes the tested all-sync timing/seek cases,
not the downstream precision gaps or the broader animation/video objective.
