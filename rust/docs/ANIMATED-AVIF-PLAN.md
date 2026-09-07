# Animated AVIF and subsequent video completion

User objective (2026-09-07): **Get animated AVIF encoding working, with all
metadata, transparency, and spec supported features. After that, complete
video encoding support.** This objective remains active. The initial all-intra
path is an implementation step, not the completion boundary.

Push checkpoint (2026-09-07, explicitly requested): review branch
`animation-avif-complete` in both repositories. The temporary sibling
serializer dependency is replaced by canonical zenavif git revision
`98c8a5011e1ac573770a8889a8b5a4a68587de90`. Cargo resolves the fetched package;
its manifest and all nine source files are byte-identical to the locally
verified serializer. Earlier local-path/no-push statements below are historical.
CI remains deferred; these branch names do not match push workflow filters.
Canonical optional-feature runtime verification is recorded below.

Main synchronization: merged `74d92430` quality fixes into the animation branch,
preserving the extracted 10-bit module and the newer lossless/context logic.
The combined tree passes 2616 tests, 123 C regression cases, 336 native
source-pixel comparisons and 168 native color C-byte comparisons. See
`benchmarks/main_merge_2026-09-07.md`.

Canonical runtime verification: zenavif `007e0246` corrects the legacy
decoder's drain loop, alpha scaling, color conversion and color-grid handling.
Its all-feature workspace suite passes 866/866 (nine existing skips), default
workspace passes 472 tests (ten existing ignores), explicit managed/AOM tests
pass 7/7, and all-feature library clippy passes with warnings denied. See the
canonical `benchmarks/legacy_decoder_2026-09-07.md` for the mutation witness
correction and exact scope.

Canonical SVT integration is now pushed as `c80da30d`: it pins merged SVT
`4e688a54` and matching SIMD revision `cc24398c`, enabling odd/partial mono
and alpha at every speed at 8/10 bits. Former refusal tests now verify
successful round trips at their unchanged quality floors; 18 QP-0 mono/native
10-bit streams reconstruct source-exactly with both raw decoders. Focused
SVT tests pass 26/26; the full all-feature workspace passes 867/867 (nine
existing skips), default tests/doctests 472 (ten existing ignores), and
all-feature library clippy passes with warnings denied. See canonical
`benchmarks/svt_capabilities_2026-09-07.md`. Public canonical animation and
explicit lossless wiring remain required; transparent grids remain unsupported.
CI is still deferred and the full objective remains active.

Canonical animation integration is now implemented locally in `8bcfb62a`
(not pushed): RGB8/RGBA8/RGB16/RGBA16 entry points and codec traits share the
still pixel-coding path, emit full headers and preserve millisecond timing,
metadata and alpha. Premultiplied-alpha signaling is corrected in still and
animated output. Four animation tests plus 26 still tests pass; 108 coded
sample pairs agree through both raw decoders. Full all-feature workspace
nextest passes 871/871 (nine existing skips); default tests/doctests pass 472
(ten existing ignores); clippy and scoped formatting pass.

The required before/after broader gates exposed existing zenravif envelope
drift: 49 ladder tolerance failures both times, with all 33 byte/quality rows
identical, and the same two screen/q80 speed inversions. Determinism and 56/56
reference conformance pass both times; the absent sibling CLI's optional armed
leg was explicitly not run. Preserve thresholds and investigate before another
push/CI. See canonical `benchmarks/svt_animation_seam_2026-09-07.md`.
Codec loop-count forwarding and MDCV conversion are now corrected locally in
canonical `1d704319` (not pushed). Both zenravif and SVT honor counts through
u32::MAX without changing sample bytes, offsets, or media timing; repeated
presentation overflow errors before mutation. Independent tests also corrected
RGB/GBR primary ordering in both encode/decode adapters and animation's incorrect
chromaticity/luminance scales. Full workspace nextest passes 875/875 (nine
existing skips), default tests/doctests 472 (ten existing ignores), encode-only
loop test 1/1, and all-feature library clippy/scoped formatting pass. Evidence:
canonical `benchmarks/codec_playback_hdr_2026-09-07.md`.

Canonical track/poster metadata correction is saved locally as `3ec2d45f`
(not pushed). Animation codec probes and frame info use track HDR, including
its absence. Track alpha premultiplication is retained independently of item
references and drives managed/AOM frame conversion. Valid no-poster sequences
probe their first sample. Tests cover conflicting poster/track declarations,
absent HDR, no poster, both alpha directions and 8/10-bit decoded pixels.
All-feature nextest passes 877/877 (nine existing skips); default tests/doctests
pass 473 (ten existing ignores); focused final tests pass 13/13; clippy and
scoped formatting pass. Determinism and 56 reference conformance cells pass;
optional armed CLI leg remains explicitly unrun. Ladder has the identical 33
byte/quality mismatch rows plus 15 timing misses (48 total); the same two speed
inversions persist. See canonical `benchmarks/track_metadata_2026-09-07.md`.

Fresh `jj git fetch` confirms SVT main remains `74d92430`, already an ancestor
of this branch. No additional main integration is needed at this checkpoint.
Canonical `c4225387` (local, not pushed) now corrects track/poster geometry,
depth/chroma and CICP/ICC source selection, including fallback matrices used
by managed/AOM pixel conversion. Track-aware constructors apply animation
limits to the track; codec lazy and native eager convenience entry points are
wired. An in-limit 150×150 track is no longer rejected by a larger 10-bit poster.
Parser primary-item absence no longer inherits track codec/color properties.
Tests include real different-size/depth posters, both ICC-absence directions,
and a colored matrix witness whose rendered pixels fail if poster selection
is restored. All-feature nextest passes 880/880 (nine existing skips); default
tests/doctests pass 475 (ten existing ignores). Final focused tests pass 16/16
and library clippy passes after the last eager-convenience routing change.
Formatting and whitespace checks pass. Determinism and 56 reference conformance
cells pass; optional armed CLI coverage is explicitly unrun. Ladder retains the
same 33 byte/quality rows plus 15 timing misses (48 total); the same two speed
inversions persist. See canonical `benchmarks/track_color_geometry_2026-09-07.md`.

Canonical `d9732407` (local, not pushed) corrects coexisting ICC/nclx loss.
Primary items and color sample entries now retain both independently of box
order; preferred-color accessors preserve ICC and new nclx accessors retain
matrix/CICP values. Managed/AOM/legacy conversion and row streaming use nclx
hints without dropping ICC metadata; the lightweight probe reports both.
Borrowed/owned and no-poster parser paths are covered. Real colored fixtures
verify rendered rows against a nclx-only control; a discard-nclx mutation fails.
All-feature workspace nextest passes 881/881 (nine existing skips); default
workspace passes 476 tests/doctests (ten existing ignores). Final expanded
focused tests pass 18/18 all-feature and 2/2 default-feature tests, including
streaming and posterless cases. Production code did not change after the full
suite. Clippy/formatting, determinism and 56 reference conformance cells pass;
optional armed CLI coverage is explicitly unrun. The same 33 byte/quality rows
plus 16 timing misses (49 total), and the same two speed inversions remain.
See canonical `benchmarks/dual_colr_2026-09-07.md`.

Next metadata audit: track spatial/Exif/XMP properties versus poster.
read_stsd skips spatial properties, read_tkhd skips its matrix and read_trak
skips track-local meta. Current libavif read.c parses sample-entry property
containers and selects them for track transformations; it reads track-local
meta for Exif/XMP. This provides an implementation reference, but independent
runtime verification of our output is still required. Auxiliary gain-map/depth
ICC/nclx coexistence and source-encoding detail provenance remain open too.
The full goal and quality-envelope investigation remain active; push/CI remain
deferred.

Next: canonical repetition/options/non-ms timing, mono animation, the existing
quality-envelope drift, and the full remaining feature inventory. The full
objective remains active.

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
| Frame timing, duration, count, seeking | All-sync timing verified through u32-max sample durations, totals above 32 bits and 257 frames; reverse/alternating seeks and reset replay preserve all color/alpha bytes. Full finite counts and exact ticks survive canonical native decode; shared codec trait precision remains |
| Finite/infinite repetition | Writes edit lists and finite/infinite presentation durations; 18 libavif track/poster checks pass. Canonical parser/native metadata preserves full-width finite counts; the shared codec u32 boundary is checked explicitly. Local serializer dependency still needs a final verified git pin before landing |
| Alpha | 8-bit straight/premultiplied associations and poster alpha verified, alongside color-only output. 8/10-bit lossless color/alpha and monochrome source pixels are verified; 12-bit and opaque/missing-alpha policy remain |
| Metadata | ICC/Exif/XMP/CICP/CLLI/MDCV/AMVE/CCLV wiring covers color track and poster. Libavif verifies exact ICC/Exif/XMP plus CICP/CLLI; independent box traversal verifies MDCV values/placement. AMVE/CCLV byte layout and track/item placement are independently checked; REVE/NDWT, camera metadata and the broader audit remain |
| Spatial properties | Square-pixel aspect, clean aperture, rotation and mirror metadata wired to color track/poster; uncropped secondary shares encoded samples and retains alpha/metadata. Displayed transformed-alpha pixel verification remains |
| Format coverage | 8-bit and native 10-bit 4:2:0 and monochrome APIs, including native alpha. Native monochrome/alpha and 8-bit monochrome support partial SBs at every preset. Native monochrome mode decisions still use the upper eight bits; full native MD, 12-bit and 4:4:4/4:2:2 remain. C's rejection of some formats does not waive this broader user objective |
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
`timing-clippy.log`. At this stage, managed/codec adapters still used legacy milliseconds; the native
wiring and exact limit accounting were subsequently completed below. The finite-count rejection is now fixed: canonical parser and native decoder
metadata preserve `u64` total playbacks, including 4,294,967,296. The narrower
zencodec adapter explicitly rejects overflow; full-width shared-codec support
remains. Libavif independently reports counts above INT_MAX as infinite, so the
gate checks exact box values for those cases. See canonical Known Bugs for the
before/after witness and API migration details.

Before eventually landing preserved workflow change `puzrqvms`, add the new
timing gate alongside its metadata gate. CI is still deferred; local serializer
path staging remains until the canonical changes can be landed and pinned.

Final timing checkpoint: all 2,590 workspace tests pass with zero skips and all
109 regression checks pass (`timing-local-final.log`). Scoped Rust formatting
and whitespace checks pass. This closes the tested all-sync timing/seek cases,
not the downstream precision gaps or the broader animation/video objective.

Monochrome continuation: `MonochromeAnimationFrame<T>` and
`encode_animation_mono[_with_options]` / `encode_animation_mono_hbd[_with_options]`
use the real monochrome pipeline for the primary track, without caller-supplied
or encoded chroma planes. They share the existing timing, repetition, metadata,
spatial properties and alpha wiring. Configuration and `pixi` report one channel;
alpha remains a separate full-range monochrome sequence without color film grain.
All frames are validated before encoding, including a test where an invalid
second frame is caught before the first could reach a refused pipeline shape.

Independent pixel checks compare decoded Y4M luma and grayscale-alpha PNG alpha
against final encoder reconstruction at 8/10 bits, qualities 40/98, strided input,
64x80 and odd 65x67, plus aligned 64x64 at 8-bit preset 1. Native fixtures have
nonzero low bits. The metadata gate now covers both monochrome and 4:2:0:
15,120/15,120 checks pass, including exact channel properties, cropped/uncropped
posters, alpha and spatial metadata. Timing and exact pixel-seek checks pass
54/54 cases across both formats (`~/tmp/animation-metadata/mono-independent.log`).
Workspace nextest passes 2,592/2,592 with zero skips and regression checks pass
109/109 (`mono-local-final.log`). Final expanded pixel/validation tests and
clippy are recorded in `mono-final-focused.log`; clippy has existing warnings.

Underlying monochrome coding restrictions remain: 8-bit partial superblocks
below preset 6 still use a clamped-root homegrown search rather than the PD0
forced-edge tree, native 10-bit requires preset >=9, and lossless is unimplemented.
These are capability gaps to implement, not reasons to call the full goal done.
The new API documents the current ranges and does not silently change presets.
CI remains deferred; no changes were pushed.

Lower-preset 8-bit monochrome continuation: partial superblocks now start at a
square coding-unit root. The new boundary search recursively visits in-frame
quadrants in decoder order, preserves the existing search on complete squares,
and compares a fitting legal single-edge rectangle against SPLIT. It uses the
C-derived binary partition rates for single-edge nodes and emits no partition
symbol for the forced corner split. Geometry splits continue past the requested
search depth when needed to reach complete coding blocks. The old clipped-root
restriction is removed only after wiring this path into the actual pipeline.

This exposed a second defect: directional neighbor construction inferred frame
height from spare superblock storage. At 72-wide aligned frames its buffer size
was not even a multiple of stride (16384 % 72 = 40), causing an assertion. The
caller now supplies only the actual aligned reconstruction canvas. The initial
reproducer refused before the feature work (`mono-low-before.log`), then exposed
that assertion (`mono-low-backtrace.log`). After both fixes, 144 aomdec cases
cover eight frame shapes, presets 0–5, and qualities 40/75/98 with strided input;
all decoded luma bytes equal final reconstruction and recon output does not
change coded bytes. The regression gate includes this witness grid. The old
facade refusal test now requires successful encoding with true dimensions,
retaining its full-superblock control; animation pixel tests also cover slow
8-bit encodes at 64x80 and 65x67 with and without alpha.

All 2,593 workspace nextest tests pass with zero skips
(`~/tmp/animation-metadata/mono-low-nextest.log`). The refusal inventory now has
52 entries. Native 10-bit's preset floor and monochrome lossless remain open;
this change does not alter presets or silently substitute another encoder.

Final lower-preset monochrome evidence: the boundary-choice unit test confirms
HORZ/VERT win on flat single-edge fixtures and the corner requires SPLIT
(`mono-low-boundary-choice.log`). The final workspace run passes 2,594/2,594
with zero skips (`mono-low-nextest-final.log`); regression passes 110/110
(`mono-low-spotcheck.log`). The full 8-bit C sweep passes 1,100/1,100 with zero
pinned differences and zero harness errors (`mono-low-identity.log` and `.tsv`).
A fresh release animation example passes all 54 timing/seek cases and all
15,120 metadata cases (`mono-low-animation-gates.log`). Final clippy completes
with existing warnings (`mono-low-clippy-final.log`); scoped rustfmt, whitespace
and the 52-entry refusal inventory checks pass. Both repositories were fetched;
remote branches were unchanged. CI remains deferred and no commits were pushed.

## Native monochrome at lower presets (2026-09-07 continuation)

The native level pass now derives RDOQ contexts from the ten-bit quantizer's
coefficient neighbor bytes, with tile-boundary resets. Faster presets retain
C's zero-context arm. It reads the actual sequence-header edge-filter flag
(monochrome disables it), and carries the parent partition into directional
prediction. Only after this wiring and decoder verification was the native
monochrome preset floor removed. Both monochrome animations and color
animations' native alpha use this pipeline without remapping speed.

Removing the floor alone exposed a real decoder mismatch: preset 0, 64x64,
quality 98 first differed at (55,24). Dav1d with in-loop filters disabled
localized 19 wrong samples before filtering. The native predictor passed
PARTITION_NONE for a VERT_A/B child, reading unavailable top-right samples.
Threading the parent partition fixes that witness. The same directional kernels
and C-derived availability tables remain in use; no prediction mode was removed.

`native_monochrome_low_presets_match_decoder` covers 108 cases: presets 0–8,
qualities 40/98, four single-tile dimensions and two dimensions with four tiles,
including odd boundaries. Every decoded aomdec luma sample equals native final
reconstruction, low input bits change coded output, and enabling reconstruction
output does not change bytes. Native monochrome animation additionally verifies
luma and alpha through libavif at speed 2, including 65x67.

This closes the preset restriction, not full native mode decision: monochrome
still selects modes from the upper eight input bits. Full ten-bit mode decision,
lossless monochrome/alpha, and the other format/video rows above remain open.
CI remains deferred; the serializer dependency is still the temporary local
path pending canonical landing and a verified git pin.

Final local verification for this continuation: **2,596/2,596 workspace tests,
zero skipped; 111/111 regression cases; 1,100/1,100 8-bit C-byte comparisons,
zero pins and zero harness errors.** Clippy completes with existing warnings;
scoped rustfmt, diff checks, and the 51-entry refusal inventory pass. Logs are
`~/tmp/animation-metadata/native-mono-{nextest-final,spotcheck-final,identity,clippy}.log`;
the failure and localization logs use `native-mono-low-*`. The last edit only
moves the pre-existing recon-canvas doc comment back onto its function after
inserting the neighbor-state type. Both canonical and main repo fetches reported
no remote changes. No push or CI run occurred.


## Lossless monochrome and alpha (2026-09-07 continuation)

The monochrome path now consumes the existing lossless 8x8 tree and codes each
leaf as four raster-order 4x4 transforms. `lossless_mono.rs` reuses the luma
prediction overlay, C-derived WHT forward/inverse kernels, QP-0 quantizer and
coefficient-rate estimator. Every transform predicts from its predecessors'
reconstruction. Candidate-local coefficient contexts are committed only for the
winner; block-skip rates omit transform syntax when the whole block is skipped.
No chroma plane is synthesized. Preset candidate limits still control search.

The monochrome lossless guard and facade flag/maximum-quality refusals were
removed after source-pixel verification. Flat fixtures then exposed a separate
color-only IntraBC guard applied to mono: `ibc_state` requires `use_funnel`, so
mono never reaches that hash search. The guard now applies to color only. When
screen tools are signaled, the existing entropy walk codes `use_intrabc=0` for
these ordinary intra blocks. All 90 decoder cases pass: ten presets, 16x16 /
65x67 / four-tile 128x128, uniform mid-gray / 0–255 checkerboard / textured
sources. Each checks exact source reconstruction and recon-flag byte invariance.

The animation tests now verify lossless monochrome luma and alpha directly
against source values, including odd dimensions and speed 2. Color animations
also exercise `with_lossless(true)` with and without alpha, including odd edges;
all decoded Y/U/V and alpha samples equal their respective source planes.

Native 10-bit WHT/TX_4X4 lossless, full native monochrome mode decisions, and
lower-preset color lossless 4x4 partition search remain open. The existing color
lossless gate passes 112/144 C-byte comparisons with 32 documented pins; all
144 decode exactly to source. These pins remain parity debt, not newly fixed
cells or decoder failures. CI remains deferred; no push occurred.

Verification: **2,597/2,597 workspace tests, zero skips; 112/112 regression
cases; 90/90 monochrome source-pixel cases.** The existing color lossless gate
reports **112/144 byte-identical +32 pinned, 144/144 source-exact**. Clippy
completes with existing warnings, scoped rustfmt and diff checks pass, and the
refusal inventory is current at 48 entries. Evidence is in
`~/tmp/animation-metadata/lossless-mono-{nextest-final,extremes,c-gate,clippy,spotcheck}.log`.
The initial full-suite failure was an old test requiring the now-supported
legacy QP-0 API to panic; it now requires exact source reconstruction instead.
Both repository fetches reported no changes; nothing was pushed or sent to CI.

### 2026-09-07: close all 32 color lossless parity pins

The lower-preset color lossless search is now wired through C's PD0 costs
(QP offset 0, real coefficient rate, transposed 4x4 WHT) and unrestricted PD1
8x8-versus-4x4 search. The per-superblock MD simulation adapts lossless
transform-type CDFs, matching C's rate estimator while the real bitstream
continues to omit those symbols. These were missing translation/wiring paths.
The strict lossless gate passes **144/144 C-byte comparisons and 144/144
source-pixel comparisons**, with every old exception removed. Three regression
witnesses cover the search, the lossless depth override, and MD CDF chaining.
Native 10-bit lossless and lower-preset color screen-content lossless remain
open. CI remains deferred.

Local verification for this change: **2,598/2,598 workspace tests, zero skips;
115/115 regression cases; 1,100/1,100 full 8-bit C-byte comparisons, no pins or
harness errors**. The lossless-specific gate is separately **144/144** with
source-pixel proof. See `benchmarks/lossless_partition_2026-09-07.md` for C
controls, captures and before/after witnesses.

Workspace clippy completes with existing warnings; scoped formatting and diff
checks pass. The refusal inventory remains at 48 entries. Both repository
fetches reported no changes; no push or CI run was triggered.

### 2026-09-07: lower-preset lossless screen content

Reproduced the preset-4 QP0 screen panic with the refusal bypassed: the fixed
partition walk sliced the source down to each block, while IntraBC's hash
query and motion search address pixels relative to the full frame. The walk
now retains the full source plane, passes an explicit block offset to the
funnel, and slices only for the older local monochrome block encoder.
Both fixed-tree pipeline call sites and recursive children use that contract.

All 96 screen/screenrep cases (four geometries, twelve presets) now match C
bytes and decode exactly to source, including the previously divergent
preset-5 cases. The lower-preset refusal is removed;
its p4/p5 regression checks now require successful C-byte comparisons, with
an additional multi-superblock repeated-content witness. The default lossless
gate includes both screen-content patterns. Native 10-bit lossless remains
open. CI remains deferred.

Final local verification: **2,598/2,598 workspace tests, zero skips;
116/116 regression cases; 240/240 lossless C-byte/source-pixel cases;
1,100/1,100 full 8-bit C comparisons, no pins or harness errors**. The
animated screen-content cases decode with exact color and alpha source pixels.
The 512x128 `screencopy` regression requires IntraBC selections in both C and
the port and exact source pixels under aomdec, in addition to byte equality.
Workspace clippy completes with existing warnings; scoped formatting and diff
checks pass. The refusal inventory has 47 entries, including 13 capability
refusals. No push or CI run was triggered.


### 2026-09-07: native 10-bit lossless

Native color and monochrome now code four 4x4 WHTs per 8x8 lossless leaf,
with native prediction overlays and coefficient contexts. Color uses the full
native funnel at every preset; monochrome retains upper-eight-bit mode
selection and produces native levels/reconstruction. The former refusal was
removed after source-pixel verification, including odd tiled frame edges.

A guard-only experiment at preset 9 produced wrong pixels (2699 B versus
C 4527 B). Full native lossless routing closes that defect. The expanded grid
also exposed a palette MDS0 lambda mismatch: the palette path used the u8
lambda while regular native candidates used C's native fast lambda. Correcting
that choice closed all nine native screen-content byte mismatches.

Measured: 168/168 color C-byte matches; 336/336 color/mono decoded-source
matches; 252 additional extreme/low-bit-only/strided/odd-tile source cases;
eight animation tests pass, including newly enabled native lossless color,
monochrome and alpha decoded against the source. Broad local gates are tracked
in `benchmarks/native_lossless_2026-09-07.md`. CI remains deferred.

The native IntraBC positive control caught an additional decoder failure:
lossless residual-bearing copies wrote a transform-partition bit C omits.
Correcting both syntax and MD cost gates makes screencopy512x128 p4 match C
at 11349 bytes and decode source-exactly (386 selected IntraBC blocks). The
regression requires nonzero residuals so skip-only copies cannot mask it.

Final local gates after that correction: 2599/2599 workspace tests with
AVIF-container enabled, 120/120 regression witnesses, 1100/1100 full 8-bit
C-byte comparisons (no pins/errors), 240/240 8-bit lossless and 336/336 native
source cases plus 168/168 native color C-byte matches. Clippy completed with
existing warnings; scoped formatting and refusal inventory checks pass
(46 entries, 12 capability refusals). No push or CI run.

Quantization-option follow-up: QP0 + QM reproduced a C defect (matching bytes,
wrong decoded source samples), while QP0 + variance boost exposed a Rust-only
syntax/quantizer mismatch. Production now uses lossless identity matrices and
passes per-SB quantizers only when delta-q is signaled, retaining both C helper
translations. The 144-case source test passes QM-only, boost-only and combined
settings across both depths, mono/color, three geometries and four presets.
Final gates after these additional fixes: 2600/2600 workspace tests,
123/123 regression cases, 1100/1100 full 8-bit C comparisons (no pins/errors),
and the variance-boost native grid at 336/336 source plus 168/168 C-byte
matches. Clippy completed with existing warnings; formatting, diff and refusal
inventory checks pass. CI remains deferred.


### Native decoded timing and limit verification (2026-09-07)

Canonical zenavif now carries exact media ticks into eager and lazy decoded
frames and exposes timing through native and concrete codec decoder accessors.
The shared zencodec frame type retains its u32 millisecond limit; the native API
is required for full precision. Two real adapter limit bypasses (fractional
milliseconds counted as zero, and long durations saturated) were reproduced and
fixed using exact cumulative ticks. Six actual-source integration tests pass,
including 548 eager/lazy decodes with unchanged pixels across six timelines.
Managed-source lib/tests clippy passes with warnings denied. Optional AOM wiring
is present but that feature and the canonical full workspace were not tested by
the isolated default-feature harness. CI remains deferred.


### AMVE/CCLV metadata completion (2026-09-07)

`AnimationOptions::amve` / `cclv` reach the color sample entry, poster and full
uncropped secondary, with shared semantic validation before encoding. The
canonical parser now preserves all four static HDR properties from color tracks
in `AnimationHdrMetadata`, independently of poster metadata. Native animation
info carries it through actual eager/lazy decode with unchanged pixels.

The expanded independent gate passes 30,240/30,240 checks across 8/10-bit,
color/mono, straight/premultiplied/no alpha and spatial/playback combinations.
It checks raw AMVE/CCLV bytes and exact associations in addition to libavif decode
because libavif ignores these two properties. Serializer 89/89, parser 20/20
plus 6 doctests, native integration 7/7, encoder nextest 2600/2600 and regression
spotcheck 123/123 pass. Evidence and constraints are in
`benchmarks/animation_hdr_2026-09-07.md`. REVE/NDWT, camera metadata and the other
requirements in the inventory remain open. CI stays deferred.
