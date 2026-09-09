> **Historical source/campaign reference, classified at handoff `0cbd1279`.**
> Use [the current handoff](../../CONTEXT-HANDOFF.md) and
> [support/identity audit](API-SUPPORT-AUDIT-2026-09-08.md) for live status.
> “Current”, “next”, “unported”, line numbers and counts below belong to their
> recorded revisions; they are not current completion claims. Where no source
> or retained artifact was recorded, the measurement is unverified historical
> prose. Preserve it, but do not use it as a merge gate or new performance claim.
> Re-locate code by symbol and the named C reference before changing behavior.

# Native research preset -1: live wiring audit

Work in progress. **Native -1 IS a verified parity mode as of 2026-09-09**, in a
stated envelope: 88/88 byte-identical over both bit depths, four content classes
and geometries including partial-superblock and odd (ceiling-chroma) dimensions
([record](../benchmarks/research_matrix_2026-09-09.meta)), now gated by
`tools/research_matrix.sh` in CI. That envelope is still/KEY, 4:2:0, SINGLE TILE,
one x86-64 host — it says nothing about tiles (tile_gate.sh does not run -1), the
HDR fork, or the Zen enhancements gated to preset == -1.
The next effort region is the Zen-only AOM-technique continuation specified in
[the encoder policy goal](../../ENCODER-POLICY-GOAL.md), after native -1 works.

Reference inspected: `reference/svt-av1` commit
`3115c0c1b23e860dfd75c94f6740e0298182dd13`. The reference tree contains the
conditional HDR hybrid; record its build mode as well as this source identity.

## Opt-in restoration-unit search (2026-09-08, local)

`ZenEnhancement::AomRestorationUnitSearch` (`aom-restoration-unit-search-v1`)
now reaches both native 8-bit and 10-bit restoration searches through the
pipeline and the existing `AvifEncoder::with_enhancement` set. It is restricted
to native research -1, all-intra 420, and disabled by default.

The AOM inspiration is `av1/encoder/pickrst.c`'s descending legal unit-size
loop. The implementation evaluates 256, 128 and 64, excludes sizes below the
actual superblock, and uses SVT's Wiener/SGR searches, per-unit coefficient/type
rate estimates, native-depth distortion and lambda. It includes the frame's
restoration type, size and UV-shift header bits in the cross-size comparison.
All planes share one selected size (UV shift 0); it does not enumerate an
independent chroma-size axis or copy AOM's normalized lambda. Ties retain the
larger size. Unlike AOM's all-NONE heuristic, it still evaluates smaller sizes
when the 256-pixel search chooses no restoration.

The C path continues to evaluate only 256. The public last-frame evidence
`last_lr_unit_size` records the selected size or None if search did not run.
Ten enabled reconstruction cases cover native8/10, odd dimensions, tile rows,
tile columns and SB128. Smaller units actually win and restore pixels at both
depths; all outputs match independent libaom reconstruction. The full workspace
suite passes 2,627/2,627, and regression spot-checks pass 136/136. Fresh
experiment-off byte gates pass 1,100/1,100 hybrid normal8 and 320/320 pristine
research cells (160 each native8/10). The completed
[144-encode canonical fleet ablation](../../../zenmetrics/benchmarks/av1_compare_2026-09-08/IMAZEN26_RESTORATION_UNITS.md)
found no measured RD gain on the two training origins. Photo QP5/12 selected
128-pixel units but became slightly larger and scored slightly lower; other
photo QPs retained 256. Screenshot search was bypassed. Keep the experiment
opt-in; this is not full-corpus calibration. A subsequent public-wrapper
equality/refusal test passed, and all 24 measured SVT cells replayed exactly
with independently matching reconstruction and recorded restoration decisions.

## Transport and current validation

`speed_config::NativePreset` checks -1 through 13. `EncodePipeline::new_with_preset`
and the identity example carry it without unsigned wrapping. The legacy
`EncodePipeline::new` and `SpeedConfig::from_preset` retain unsigned entry points.
Internal mode/partition/filter/search derivations now carry signed presets.
This does not establish that every research-only consumer is implemented.

The first signed transport revision passed all 2,612 workspace tests, including
two new representation/SGR-control checks. Those new checks are transcription
and transport checks, not C output comparisons. Before changes, the baseline
was 2,610 tests and 127/127 regression spot-checks. The signed revision also
passed 127/127 regression spot-checks; no research identity claim follows from
that normal-preset suite.

## Required research-mode differences

| Area | Verified C behavior | Live port status |
|---|---|---|
| SGR restoration | `get_sg_filter_level_allintra` returns level 1 at -1: both lanes search all 16 entries with refinement | Signed selector now reaches all-intra search controls; full enabled-output validation pending |
| Wiener restoration | All-intra -1 and 0 both use level 3 | Existing controls retained |
| CDEF | Research level differs from normal preset 0 | 8/10-bit search now honors the per-slot UV masks in both passes. Three C differential tests also check the consumed projection; 2,615 workspace tests and 127/127 normal byte regressions pass |
| Depth refinement | All-intra screen level 1; nonscreen research level 3 instead of level 6 | Signed research branches added to the live depth configuration |
| Partition geometry | `get_nsq_geom_level_allintra(-1)` selects level 1, enabling HA/HB/VA/VB | HA/HB/VA/VB generation, pruning, first-child reuse and picture coefficient-class transport added; geometry/unit tests and normal byte regressions pass. Chosen-asymmetric C-output and independent-decoder witnesses pending |
| Partition search | Low/VLow coefficients reduce the research NSQ search level | Frame coefficient class now travels through `CodingQuantCfg` to all three pipeline `NsqCfg` consumers; research Low/VLow branches are reached |
| Lambda weighting | At -1, tune 0–2 omit the normal QP-dependent weight; IQ curve and extended-CRF bump still apply | Signed `frame_lambda_weight_for_preset` now reaches main and per-SB consumers; the tile override preserves zero research weight instead of falling back to normal defaults. 2,613 workspace tests and 127/127 normal-preset byte regressions pass; enabled research output gate pending |
| Video derivations | Research branches affect additional prediction/search tools | Signed carriers alone are insufficient; audit the full default arm before video parity claims |
| Public wrappers/query | Research must be reachable and report verified support | AvifEncoder now exposes checked with_native_preset and resolved_native_preset; with_speed retains compatibility and replaces a native override. Zenavif routing/effort query wiring remains pending |

Do not alias research to preset 0, suppress its asymmetric shapes, weaken an
identity expectation, or label this table complete based on accepting -1.
Complete translation and wiring first, then run the enabled research identity
and independent decoder matrix. Normal preset regression gates remain required
throughout the implementation.

## Corpus reduction prerequisite

The canonical repository is [imazen/imazen-26](https://github.com/imazen/imazen-26).
Its [access index](https://github.com/imazen/imazen-26/blob/main/ACCESS.md) points
to authoritative split manifests with render URLs; render names cannot safely
be inferred from original names because EXIF rotation changes dimensions.
Read and pin the manifests and variant registry before declaring scouting jobs.
Existing K-sized diversity-cluster subsets are not the requested minimum
RD/RD-speed-zone set. The selection and validation requirements are in section 5
of the policy goal. No reduced RD-zone set has been measured yet.

Canonical metadata snapshot: commit `187fbf338ce08e8e6654db7f04ddae58d5263da2`,
local `/home/lilith/tmp/svt-tracking/imazen26-canonical/provenance.json` with file
hashes. Parsed coverage: train 1,084 origins (1,082 SDR URLs, 38 HDR URLs),
validate 658 (657 SDR, 20 HDR), test 418 (418 SDR, 18 HDR). Missing SDR renders
are raw DNG origins 1444/1458 (train) and 1455 (validate); do not silently omit
them from a full-corpus claim. URL presence is not a download/decode check.
The split documentation also identifies related patent scans, brochures and
website viewports across IDs; account for that leakage when fitting policies.

Additional source-audit finding: the C video default arm assigns lambda weight
300 to non-I frames at QP >= 62 (normal presets), while the shared Rust helper
currently assigns 175. Preserve this as a separate enabled-witness investigation;
research mode correctly omits both normal weights.

Asymmetric-shape implementation now follows the whole decision path: C iteration
order is N/H/V/H4/V4/HA/HB/VA/VB; incomplete blocks inject only H/V, size 8
excludes the asymmetric shapes, and size 128 excludes H4/V4. Besides geometry,
the port now carries the HA/HB/VA/VB arms of reconstruction/transform
pruning from `product_coding_loop.c`, `update_skip_nsq_shapes` (-10 coefficient-free aggressive offset),
and `update_redundant` (HB←H, VB←V, VA←HA first-child reuse). Merely generating
the three children is not full C search parity. The existing packer already
has asymmetric partition context/offset arms; validate those with chosen
asymmetric blocks rather than assuming source presence is sufficient.

Screen-tool audit found two additional signed-ladder omissions: the live
all-intra palette match excluded -1 (C uses level 2), and the IntraBC match
excluded -1 (C uses level 1). Both live ladders are now wired. The sequence-level
IntraBC mesh scaling flag is now false at all-intra MR, preserving the video
arm’s true flag. The revision passes 2,615 workspace tests and 127/127 normal
byte regression checks.

## First retained research output witness

`gradient 64x64 QP40 preset -1`, native 8-bit, reference HDR mode 0:
Rust 299 bytes, C 283 bytes. Both streams independently decode with aomdec.
They are **not byte-identical**. Sequence headers agree; the first parsed frame
header difference is luma loop filter 9 in Rust versus 0 in C. CDEF and tile
payload also differ. The first canonical tile operation difference lies in the
SGR restoration literal run; this is localization, not a proven root cause.
Do not disable restoration or asymmetric search to make this witness pass.

Retained input, streams, decoded planes, traces and verbose report:
`/home/lilith/tmp/svt-tracking/research-first/`. Normal spot-check log:
`/home/lilith/tmp/svt-tracking/research-screen-spotcheck.log`; workspace log:
`/home/lilith/tmp/svt-tracking/research-screen-nextest.log`. Full research matrix
and full pre-landing identity sweep remain outstanding.

The first witness is now fixed (local jj `myzskwqo`): the live `use_pd0`
dispatch still matched only presets 0..=5, sending -1 through the old fallback
partition recursion. Extending it to -1..=5 makes the witness **283 bytes and
all 2,690 tile operations identical**. Independent aomdec output also matches
C byte-for-byte. All 2,615 workspace tests pass; a before/after witness is now
in `regression_spotcheck.sh`. This is one enabled witness, not the full matrix.
The suite’s new `IF_ARTIFACT_DIR` option retains each cell’s source, encoded
streams, settings and logs, indexed in `index.tsv`, for expanded research work.

Expanded research sweep after the dispatch fix: **40/40 native 8-bit cells
byte-identical** (uniform/gradient/diag/screen, 64/128, QP5/12/20/40/63).
`research-matrix.tsv` and its artifacts under `~/tmp/svt-tracking/` retain all
40 inputs and both outputs; `artifacts/manifest.json` verifies file hashes.
The normal spot-check plus new research witness passes **128/128**.

The same native 10-bit grid initially passes **26/40**; 14 mismatches remain
in `research-matrix10.tsv` with retained artifacts. The 10-bit lambda builder
still used the normal frame weight with no preset input. A separate local
revision carries the signed preset into all three consumers (leaf funnel,
partition search and re-encode) and suppresses that weight for research. Its
validation is pending; do not infer 10-bit parity from the 8-bit matrix.

The native 10-bit lambda fix now passes the same **40/40** research grid,
including all 14 previously failing cells. All 2,615 workspace tests pass.
The native10 gradient64 QP40 failure (265B Rust / 277B C before, 277B exact
after) is added to the regression gate. Independent decode validation of the
80 retained pairs is recorded in `research-independent-decode.json` and its
log; the enlarged spot-check is running in `research-bd10-lambda-spotcheck.log`.
The coverage tool’s historical normal-preset matrix still contains real-image
and partial-frame divergences; these research synthetic successes do not
supersede that evidence or satisfy the full pre-landing gates.

Geometry expansion: **118/120** native 8-bit cells match (15 dimensions,
gradient/screen, QP5/12/20/48). Failures: gradient192x192 QP12 (C6480B/Rust6516B)
and gradient512x512 QP48 (C3349B/Rust3334B). Artifacts and rows are under
`~/tmp/svt-tracking/research-dims8*`. The enlarged regression suite passed
129/129 before these new changes.

The 192x192 witness has byte-identical unfiltered reconstruction through SB5.
In SB6, C chooses VertA at mi=(36,4), Rust chooses Vert; reconstruction remains
identical but syntax adaptation differs and changes SB7 decisions. C's actual
`update_stats` inputs were captured with the new `SVT_MDSTATS_OUT` interposer
(the stream remains byte-identical to the uninstrumented C stream). This
corrects the initial apparent first divergence at SB7: matching reconstructed
pixels alone does not establish matching partition/mode decisions.

Source audit found directional prediction consumers passing PART_NONE to
neighbor-availability tables even for VertA/B children. Current revision
threads `UnitGeom.partition` through whole-block and transform-overlay
prediction at both bit depths. In the failing VertA node, child0 costs agree
exactly and child1 costs diverge; the vertical-specific availability tables
are relevant there. Validation of this correction is running; do not yet
claim it resolves either geometry witness.

Canonical imazen-26 smoke inputs downloaded: train IDs1000 (photo4032x3024)
and8100 (screenshot1440x900), authoritative PNG-v3 SDR URLs. Download SHA256,
metadata commit and dimensions are in `~/tmp/svt-tracking/imazen26-smoke/manifest.json`.
These two inputs are pipeline smoke tests, not a measured representative set.

The directional-availability correction resolves both retained geometry
failures: 192x192 QP12 is 6,480B / 51,825 tile ops exact; 512x512 QP48 is 3,349B
/ 41,859 tile ops exact. All 2,615 workspace tests pass. Both failures are now
explicit regression cells; the 131-cell spot-check and full 120-cell dims8
repeat are running. Fresh outputs use `*-fixed` artifact paths.

The directional fix passes the enlarged regression suite **131/131** and the
full repeated 8-bit geometry grid **120/120**. All 120 retained C/Rust stream
pairs independently decode identically (`research-dims8-independent-decode.json`).
Native10 geometry and the initial canonical imazen26 smoke encodes are now
running serially. Tracking issue21 was updated with local progress and the
remaining API/policy/routing/corpus requirements; no new remote commits were
found by `jj git fetch`, and no push or CI was started.

Native10 geometry finishes **117/120**, with three screen QP48 differences:
256x256 C540B/Rust537B,384x256 C569B/Rust573B,512x512 both1012B but different
bytes. All have retained artifacts under `research-dims10/artifacts`; the
smallest witness is `cell.0oNapxbn`, whose parsed headers agree and tile payload
is C511B/Rust508B. Do not infer identity from equal byte counts. Canonical
imazen26 smoke encodes continue in the same serialized job after this sweep.

The first canonical imazen26 smoke grid passes **8/8 C-byte comparisons**:
train1000 photo and8100 screenshot,512x512 center crops, QP5/12/20/48.
Rows and retained streams: `research-imazen26-smoke*`. This verifies only two
real inputs; it is neither a representative subset nor an RD/performance study.

The smallest native10 screen failure is localized further in
`research-screen256-q48/`. Both encoders choose IntraBC at pixel(192,144),
16x16, but C chooses transform depth1 and Rust depth0. C MDSTATS instrumentation
now records the actual IntraBC flag as well as mode/geometry; instrumented C
output remains byte-identical to the original C output. The two selected
choices have different costs/distortions; compare matching candidate/depth
evaluations before attributing this to distortion scaling. C full-cost logs
and Rust candidate logs are retained. No fix for these three native10 screen
failures is claimed yet.

Native10 screen root cause found: the first actual difference is the copy
vector at pixel(128,96), C(-768,-384) versus Rust(-768,-504), in eighth-pixel
units. This precedes the transform-depth difference at(192,144). C's
`mode_decision.c::intra_bc_search` forces 8-bit search pixels and SAD LUT,
but derives `errorperbit` from `full_lambda_md[hbd_md]`. Rust retained the
8-bit frame lambda for that vector-cost term. Local vlsuktxv now uses the
native10 lambda at the search consumer. The 256x256/QP48 witness is **540B /
18,355 tile ops exact** and all 2,615 workspace tests pass. A measured
regression cell is added; the 132-cell spot-check and 120-cell native10 geometry
repeat are running. Do not change the pixel/SAD domain to 10-bit: C explicitly
keeps those 8-bit. Fresh evidence uses `research-dims10-fixed*`.

Native10 vector-cost fix passes **132/132** regression checks and the repeated
**120/120** native10 geometry grid, including all three former failures.

Public AvifEncoder now accepts `with_native_preset(NativePreset::RESEARCH)`
and reports `resolved_native_preset()`. Its normal speed mapping is preserved;
last preset/speed selection wins, and native presets10..13 canonicalize to9
as in C all-intra. Tests verify research output equals direct signed-pipeline
output and differs from preset0 at both output depths. The wrapper's existing
u8 input is widened for10-bit output; nativeu16 input remains a pipeline API.

A new partial-frame decoder witness exposed a stale wrapper-only refusal of
odd dimensions. The wrapper now validates tightly packed chroma with rounded-up
half dimensions and accepts65x67. Independent aomdec output agrees with
pipeline reconstruction at8/10bits; shortened odd-width chroma still errors.
All **2,618 workspace tests pass**. Doctests and the final wrapper spot-check
are running. No parity expectations were loosened; the former odd-size
refusal test now checks rounded-up chroma bounds and successful encoding.

Research wrapper validation completes: **3/3 doctests and132/132 regression
checks pass**. The65x67 wrapper decoder witness is now retained by the regular
spot-check loop (next total133).

Research tile coverage completes **18/18 at8-bit and18/18 at10-bit**, all with
successful independent aomdec decoding. Geometry512x384, QP12/48, tile rows and
columns log2=0/1/2. Each depth has16 nonzero-grid cells whose C output differs
from its untiled control; the two0/0 cells are baseline controls. These test
ragged tile rows as well as multiple columns. Evidence:
`~/tmp/svt-tracking/research-tiles{8,10}.tsv` and corresponding artifact trees.
`tile_map.sh` now accepts preset/QP/depth overrides and optional per-cell
artifact retention, while preserving its existing default grid and scoreboard
semantics. Shell exit alone is not the parity gate: inspect every verdict,
decode result and anti-vacuity control.

All120 retained native10 geometry pairs also independently decode to identical
native10 samples (`research-dims10-independent-decode.json`). This closes the
previously pending decode comparison for that grid.
The static comparator in sibling zenmetrics now accepts checked Rust -1;
its five library tests pass, including C/Rust byte identity with actual10-bit
samples containing nonzero low bits. Static PIE/no dynamic NEEDED entries
was rechecked on the rebuilt measurement binary. Broader real RD/parity
measurements are separate from these correctness witnesses.

The updated spot-check completes **133/133**, including the public odd-size
research wrapper witness. Canonical imazen26 RD baseline completes150/150
encodes (50cells, three rounds); native -1 matches C on10/10 distinct cells
and30/30 repeated pairs, using full-image resizes of training1000/8100.
See sibling zenmetrics `benchmarks/av1_compare_2026-09-08/IMAZEN26_RESEARCH_BASELINE.md`
for size/quality/time, conversion ceilings and exact source/build provenance.
The screenshot's Rust -1 is5.4xC time atQP20 despite byte identity; profiling
is in progress. All five libaom/Rust AOM screenshot cells differ, recorded
separately. Neither observation licenses disabling SVT's research tools.

## Zen continuation: explicit AOM intra-edge experiment (2026-09-08)

`ZenEnhancement::AomIntraEdgeFilter` (`aom-intra-edge-filter-v1`) is now
available through `AvifEncoder::with_enhancement` and `EncodePipeline.enhancements`.
It requires native -1, all-intra and 420, and defaults off. It is an isolated
experiment on top of the named reference, not C parity or a calibrated effort
bundle. The override reaches both sequence signaling and the funnel after
`intra_arm::apply`, including native10 prediction. The identity runner accepts
`SVTAV1_ZEN_INTRA_EDGE_FILTER=1`; the static comparator records the explicit
reference and override and verifies serialized settings replay the same output.

Enabled geometry coverage exposed a real chroma reconstruction defect:
128x128, QP20, native8, two tile rows differed at final UV byte17397. Luma
was exact, and dav1d with filters disabled showed the first coding-order error
in chroma prediction for the 8x16 luma block at (24,32). The above 8x8 group
was split into 4x4 luma blocks: the (28,28) chroma owner had smooth UV mode9,
but `filt_type_uv` read the non-owner at (24,28), mode2. Choosing the correct
column alone fixed QP20 but still failed QP48: luma-only children overwrote
the UV neighbor maps before their group's owner predicted chroma.

The correction records UV modes only for chroma owners, across their complete
8x8 luma group, selects bottom-right owner coordinates and excludes unavailable
neighbors at rounded chroma tile boundaries. This follows
`adaptive_mv_pred.c::svt_aom_init_xd` and `enc_intra_prediction.c::get_filt_type`.
No angular modes, partition shapes or assertions were disabled.

Checks after the correction: 2,626/2,626 workspace tests, zero skipped;
136/136 regression spotchecks.
`zen_intra_edges_at_partial_frames_and_tiles` checks 36 off/on encodes across
8/native10, QP0/20/48, odd65x67 and128x128 row/column tile boundaries; all
independently decoded pixels equal encoder reconstruction. A separate64x64
witness proves the enabled option actually changes reconstructed pixels and
is reachable through the public wrapper. Both are in regression_spotcheck.
Refreshed parity passes1,100 hybrid normal8,1,100 pristine normal8 and320
pristine research cells (160 per native depth).

The first120-encode canonical photo/screenshot ablation completed on the
PRE-FIX encoder, so it does not establish the corrected implementation's RD
tradeoff. Its corrected120-encode replacement is complete; all20 SVT cells
also pass exact-byte/encoder-reconstruction replay. At bracketed SSIM2 targets
70/photo and80/screenshot, filtering costs1.4%/1.7% more bytes and extra time.
Keep it opt-in. Results and durable artifact location:
`../../../zenmetrics/benchmarks/av1_compare_2026-09-08/IMAZEN26_INTRA_EDGE.md`. Artifacts: ~/tmp/av1-imazen26-intra-edge-2026-09-08;
logs: ~/tmp/svt-tracking/zen-edge-{mismatch-trace,chroma-owner-map-fix,
owner-final-nextest,owner-spotcheck}.log. The larger policy/routing/corpus goal
remains open.
