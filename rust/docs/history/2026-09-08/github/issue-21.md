# Historical GitHub issue #21: Tracking: C SVT feature parity, zenrav1e gaps, and AVIF/video completion

Snapshot before the 2026-09-08 handoff cleanup. State at capture: **OPEN**.
Original: https://github.com/imazen/zenav1-svt/issues/21. Current disposition: [issue audit](../../../OPEN-ISSUES-AUDIT-2026-09-08.md).

## Original body

This tracks completion of the C SVT translation and consumer wiring, useful feature gaps relative to zenrav1e, and the remaining animated AVIF/video work. Snapshot: 2026-09-08. This updates the roadmap in #7 without closing or silently superseding its unresolved requirements.

Priority: correctness and real-world usefulness first, then features zenrav1e exposes that zenav1-svt does not. Complete translation and production wiring before claiming parity from a harness. Optional/default-off features count.

## Current landing state

| Repository | Verified/pushed state | Landing status |
|---|---|---|
| zenav1-svt | Main `8e6f9af4` | Merged; CI green |
| zenrav1e | Master `1447c200` | Partial-chroma distortion/sub8 inter correctness fixes merged; CI and fuzz green |
| cavif-rs | Review `7f55b540` on `animation-avif-complete` | Functional checks pass; main withheld pending quality clearance |
| zenavif | Review `28d08564` on `animation-avif-complete` | Functional checks pass; main withheld pending quality clearance |

SVT merged-code evidence: 2,616 tests, 123 C regression cells, 336 native lossless source comparisons, and 168 native color C-byte comparisons. Canonical wrapper evidence: 925 tests including doctests; independent checks of 40 color-format files / 80 color frames, with source-exact RGB identity and alpha checks. These are bounded verification results, not proof of every feature combination.

Update: SVT review branch `animation-avif-complete` is now [`add618a1`](https://github.com/imazen/zenav1-svt/commit/add618a12c4c650588a4d70342919a9764107063). It contains the mainline tune sharpness fix (`11a6e4e2`) and forced SCM 0/1 classification fix. Combined validation: 2616/2616 workspace tests, two doctests, 127/127 C regression cells, three additional native 10-bit C-byte witnesses, and all 16 C/Rust witness streams accepted by libaom. Both defects were reproduced before correction. These commits are **pushed for review, not on main**; full-workspace lint debt remains and CI is still deferred. Higher-level wrapper exposure is still open. Evidence: [tune](https://github.com/imazen/zenav1-svt/blob/add618a1/rust/benchmarks/tune_sharpness_2026-09-08.md), [screen controls](https://github.com/imazen/zenav1-svt/blob/add618a1/rust/benchmarks/screen_controls_2026-09-08.md).

## C SVT versus zenav1-svt support

“C SVT” below denotes the pristine v4.2.0 shipping envelope. The historical differential oracle is hybrid3115 MODE0; it is not identical to pristine C throughout that envelope. Source and HDR build mode must be named separately (see the reference audit below). “Implemented” means a production path exists in the stated envelope; it does not imply universal C bit identity or exposure by every wrapper. AVIF container features are listed separately because they are not elementary AV1 encoder features.

| Feature | C SVT reference | zenav1-svt | Remaining work / qualification |
|---|---|---|---|
| 8-bit 4:2:0 still/all-intra encoding | Supported | Implemented, C-byte and decoder gates | Preserve enabled-tool coverage across presets and partial dimensions |
| Native 10-bit 4:2:0 | Supported | Implemented, including odd/partial dimensions and lossless | #18 encoder fixes landed; consumer/fleet rollout and replacement of affected outputs still need verification |
| Monochrome and alpha planes | No accepted monochrome encode mode in pinned C | Rust extension, 8/10-bit including odd sizes and lossless | Native mono mode decisions still use upper eight bits; full native-depth mode decisions remain |
| 4:2:2 / 4:4:4 / 12-bit input | Rejected by pinned C settings validation | Unsupported by SVT backend | Extensions, not missing shipping-C translations; zenrav1e provides a useful comparison/priority |
| QP/CQP and single-still CRF behavior | Supported; CRF and CQP coincide in verified single-still cases | Implemented in still envelope | Do not misclassify single-still CRF equivalence as missing rate control |
| Multi-frame CRF/VBR/CBR, lookahead and temporal filtering | Supported in video | Incomplete end-to-end | Wire and verify real multi-frame consumers; helper translations alone are insufficient |
| Public frame submission, packet output, drain | Supported | Public `Encoder` remains a scaffold | `send_frame` discards data and `receive_packet` returns `NotReady`; replace with a working public video path |
| Inter prediction, GOPs, reference management | Supported | Experimental production machinery and narrow C/decoder gates | Broader GOP/reference/refresh behavior, global motion and inter-lossless limits remain; not general video support |
| Lossless still coding | Supported, with recorded reference edge defects | Implemented for 8/10-bit color/mono/alpha | Extend combination coverage; superres/inter restrictions remain; use source-exact decoder checks where C itself fails |
| Palette / IntraBC screen tools | Supported | Implemented in tested color paths, including native lossless witnesses | Higher-level preference forwarding and wider combinations remain |
| Screen-content and tune controls | Supported | Partial wiring | #17: SCM 0/1 and mainline tune-0 sharpness fixed on review `add618a1`; wrapper exposure remains. SCM 3 can legitimately equal the default |
| Deblocking / CDEF / Wiener restoration | Supported | Implemented in verified envelopes | Superres plus active restoration is refused; preserve video-specific level selection |
| SGR / switchable restoration | Live in video presets 0–3 and all-intra MR | Native signed −1 and video search/syntax/apply wired in tested cases | Wrapper −1 is reachable; retain enabled-tool, HDR/tune and wider real-image coverage |
| Restoration-unit size search (stills) | Fixed 256-pixel units | Opt-in Zen search over legal 256/128/64 sizes; native default remains 256 | Fully wired and decoder-checked at native8/10, odd/tile/SB128 boundaries; 144-encode ablation found no RD win on these two training origins, so no automatic default |
| Super-resolution | Supported | Restricted 8-bit path, including tested grain combinations | Native 10-bit, active restoration, and lossless combinations remain refused; assess valid C/spec semantics per combination |
| Film-grain estimation, denoising, supplied tables, synthesis | Supported | Translated and wired for existing 8/10-bit 4:2:0 envelope | 28 valid C streams byte-identical; 29 decoder-exact reconstructions. Wider inter lifecycle and wrapper control exposure remain |
| Photon noise / HDR-fork tuning and variance boost | HDR fork, not all mainline C features | Implemented in tested paths | Audit each public control and valid combination; do not conflate fork-only behavior with mainline tune behavior |
| HDR/color signaling | C syntax/configuration support | Implemented supported signaling and AVIF metadata paths | Finish wrapper exposure and association coverage; container completeness is separate |
| Parallel encoding | Supported | Deterministic tile parallelism with thread bound | Not equivalent to C's complete video scheduling/throughput architecture |

## Remaining work, ordered for usefulness

- [ ] **Correct ignored public controls (#17).** Enabled C-output regressions and pipeline fixes for SCM 0/1 and mainline VQ sharpness are pushed at `add618a1`; finish wrapper forwarding and landing. Preserve legitimate default-equivalent settings.
- [ ] **Clear the wrapper quality merge blocker.** Last full gate reported 32 non-timing changes, 20 timing misses and six speed-order inversions; these are not a count of independent bugs because symmetric gates also flag improvements. Current low-quality witness `s2/mixed/q15` is 842 bytes / SSIM2 31.245 versus historical 874 / 48.023. The first major drop is isolated to enabling horizontal/vertical partition candidates. Current reconstruction equals libaom; a bottom-up diagnostic did not fix the quality gap. Investigate scoring/decisions with matched-quality bitrate and repeated timing; do not disable tools, roll back working features, or relax baselines to hide failures.
- [ ] **Land the verified wrappers.** Run local quality, determinism and conformance gates on the exact final dependency chain. Merge cavif-rs first, then zenavif with the merged pin. Trigger CI only after local clearance and verify the final remote revisions.
- [ ] **Make public inter-frame video useful.** Replace the submission/packet scaffold; implement frame ownership, queueing, timestamps, draining and errors. Exercise changing frames through real GOP/reference management and independent decoders. zenrav1e already offers a functioning inter-frame encoder; prioritize this practical gap over obscure metadata additions.
- [ ] **Complete video feature wiring.** Rate control/lookahead/temporal filtering, reference/GOP variants, global motion, restoration selection and inter-lossless must reach real callers. Keep each supported envelope explicit until tested.
- [ ] **Expose implemented coding tools through consumers.** Film grain and palette/IntraBC preferences need public wrapper forwarding; verify enabled output and lifecycle, not only configuration storage.
- [ ] **Improve practical format parity with zenrav1e.** Assess 4:4:4/RGB identity for screen/text fidelity, 4:2:2 and 12-bit, and native mono decisions. These extend pinned C's format envelope and need source/reconstruction/conformance oracles rather than a patched-C claim of shipping parity.
- [ ] **Lift useful combination restrictions.** Native 10-bit superres and active-restoration combinations; classify lossless/superres semantics carefully. Reconcile the generated refusal inventory with actual reachable callers rather than counting each refusal as a separate feature.
- [ ] **Complete animation control/timing interfaces.** Exact ticks work in native APIs and the concrete codec adapter; the generic zencodec animation trait still takes milliseconds. Add independent per-frame hint maps and real inter hint application; current shared hints are intra-only.
- [ ] **Finish broadly useful AVIF surfaces.** Transparent grids across backends; remaining display transforms (fractional clean aperture, track matrix and non-square-pixel presentation); bounded streaming, fallible allocation and cancellation inside muxing.
- [ ] **Complete advanced AVIF spec coverage.** Gain-map/depth auxiliary images/tracks, metadata associations/provenance, collections/layers/entity groups and remaining sample transforms. Maintain an explicit implemented/unsupported inventory with independent parser/decoder evidence; full specification coverage is not yet established.
- [ ] **Recheck quality-axis behavior (#19).** Reproduce the named adjacent-setting inversion on current consumers and document effective QP saturation. A smaller, lower-quality output is not automatically a dominated rate-distortion point.
- [ ] **Resolve existing full-workspace lint debt separately.** Broad all-target clippy on this toolchain reports copied-C literal precision, documentation placement and mechanical style diagnostics. Do not change transcribed constants to silence precision warnings or let a general cleanup replace feature work. Functional and C parity gates remain independent.
- [ ] **Finish deployment and documentation follow-through (#18, #8, #4).** Verify consumer/fleet rollout and affected-output replacement, reconcile stale capability/count claims, and check the remaining fresh-machine/build/performance requirements before closing those issues.

Already implemented animation surfaces include RGB/RGBA storage inputs, explicit 8/10-bit coding, alpha/premultiplication, exact native/concrete-adapter timing, loops, common ICC/Exif/XMP/color/HDR metadata, and tested poster/track associations. Canonical zenrav1e-backed animation also now forwards 420/444, RGB identity and full/limited range. This does **not** make those formats available through the SVT backend.

## Acceptance criteria

- A feature is complete only when configuration reaches production implementation, syntax, lifecycle and the consuming API.
- Optional tools must be tested enabled, including a witness that detects removing the wiring.
- Use C differential checks inside C's accepted envelope, independent decoder/reconstruction checks throughout, and source-exact checks for lossless. Record C reference defects explicitly.
- Keep functional correctness, byte identity, rate-distortion quality and runtime measurements distinct. Do not convert historical regression counts into a bug count without classification.
- Preserve unsupported-configuration errors until the implementation and tests justify lifting them. No silent fallback, discarded controls or expectation relaxation.
- Record exact revisions and evidence as items land; closing this tracker requires resolving or explicitly scoping every remaining item, not merely passing the still-image suite.

## Evidence and related work

- [Open issue audit](https://github.com/imazen/zenav1-svt/blob/8e6f9af4/rust/docs/OPEN-ISSUES-AUDIT-2026-09-07.md): #4, #7, #8, #17, #18, #19.
- [Animation/video plan](https://github.com/imazen/zenav1-svt/blob/8e6f9af4/rust/docs/ANIMATED-AVIF-PLAN.md) and [inter implementation plan](https://github.com/imazen/zenav1-svt/blob/8e6f9af4/rust/docs/INTER-ENCODE-PLAN.md). Historical checkpoints are not current completion claims.
- [Film-grain translation, production callers and enabled gates](https://github.com/imazen/zenav1-svt/blob/8e6f9af4/rust/docs/film-grain-port-map.md).
- [Wrapper quality investigation](https://github.com/imazen/zenavif/tree/animation-avif-complete/benchmarks/quality_drift_2026-09-07) and [animation color verification](https://github.com/imazen/zenavif/blob/28d08564/benchmarks/animation_color_2026-09-07.md).


## Research preset and policy work — local progress, 2026-09-08

These changes are local and are not on main. CI remains deferred pending the full local gates. The latest fetch found no new remote changes.

| Feature | Pinned C reference | Current local Rust implementation | Remaining evidence |
|---|---|---|---|
| Native research preset −1 | Accepted by the public validator | Checked signed preset reaches pipeline, AVIF wrapper and comparator; −2/−3 remain rejected | Complete enabled-tool, HDR/tune and real-image matrices |
| Research partition search | Asymmetric HA/HB/VA/VB; research depth/coeff controls | Shape generation, pruning, reuse and directional availability wired; geometry, native10 and tile reruns completed | Wider real images and reference-specific optional combinations |
| Research filtering and screen tools | Full SGR search, research CDEF UV candidates, palette2/IntraBC1 | Live consumers wired at 8/10-bit | Broader independent decode and format coverage |
| Research lambda policy | Omits the normal QP-dependent frame weight | Signed preset reaches 8/10-bit mode and partition costs | HDR/tune combinations and video audit remain |
| Zen continuation beyond −1 | Not a native C preset | Opt-in AOM intra-edge policy reaches prediction/signaling; corrected 120-encode ablation and reconstruction replay complete | Other adoption candidates, broader calibration and fractional-effort policy remain; no automatic bundle |
| Continuous effort / strict SvtParity / backend routing | Not a C API | Defined in the local encoder-policy goal | Resolved policy, actual routing and calibration remain |
| imazen-26 RD/RD-speed representatives | Not a codec feature | Canonical metadata pinned; two train images downloaded for smoke tests | Corpus scout, measured zone selection and full held-out validation remain; no representative subset claimed |

Measured corrections include −1 bypassing the full partition dispatcher, native10 retaining normal lambda weights, VertA/B using PART_NONE availability, and native10 IntraBC using the 8-bit MV-cost lambda. Hybrid research coverage reached 40/40 synthetic plus 120/120 geometry cells per native depth, and 18/18 tile cells per depth with independent decode witnesses. The public wrapper accepts signed −1 and odd 420 dimensions. Current local checks pass 2,626 workspace tests and 136/136 regression checks.

### Explicit pristine versus hybrid source selection

Pristine v4.2.0 is `9292ec8e32bce26f781f277ec8739b53426c4300`; the historical hybrid pin is `3115c0c1b23e860dfd75c94f6740e0298182dd13`. A fresh pristine build found five mismatches in the 1,100-cell default grid despite Rust matching hybrid MODE0 throughout. Controlled source ablation isolated the cause: pristine independent-chroma presorting uses variance, while the hybrid uses SAD even in MODE0. Native10 variance also requires C's separately rounded SSE and signed sum.

The local Rust implementation now exposes `SvtReference::{Mainline420, Hybrid3115}` through the pipeline and AVIF wrapper and carries it into the real chroma search. Existing constructors retain the hybrid behavior. Mainline selection refuses hybrid-only controls and monochrome extensions. This is the reference foundation; the full strict `SvtParity`/effort/resolved-policy API remains open.

| Source-specific check | Completed result |
|---|---:|
| Pristine normal8 synthetic+dims | 1,100/1,100 byte-identical |
| Pristine native−1 synthetic+dims | 320/320; 160 each at8/10-bit |
| Legacy hybrid normal8 compatibility | 1,100/1,100; zero pinned exceptions or harness errors |
| Distinct reference behavior | Five normal cells and32 research cells differ from hybrid |
| Independent decoding | 846 unique streams covering all1,420 cells; zero failures |
| Dual-reference wrapper/native regression | Both sources, native8/10 and−1/0; decoder reconstruction checked |

See local `rust/docs/PARITY-REFERENCE-AUDIT-2026-09-08.md`, the two `rust/benchmarks/pristine-reference*.json` summaries, and committed test fixtures. The C pin and original parity expectations were not changed. MODE0 also retains a larger validator domain (fps, tune6, curve3, floating QP compression), so a reference-mode switch alone cannot establish unrestricted mainline parity.

### Measured speed work and dependency update

Batched IntraBC mesh SAD preserves candidate order and bytes. On the canonical imazen-26 training screenshot8100, full-image resize512×320, native−1/QP20: **14,292B /84.101 SSIMULACRA2**, Rust median **13.528s →6.106s**; C control **2.504s →2.503s**. Three timing rounds per cell, one host; this is one screenshot witness, not a corpus-wide gain. The photo control was essentially unchanged. Published archmage/magetypes0.9.29 replaced temporary Git patches across SVT/probes, the static comparator and zenavif. Its repeated screenshot result is **6.114s**, identical bytes/quality (C2.507s). Comparator5tests, zenavif27tests and six standalone probe builds passed on that dependency graph. No RD or speed gain is claimed for the subsequent reference-metric correction.

These bounded results do not resolve the older real-image parity gaps, prove full C translation, establish backend routing policy, or make the branch ready to land. The canonical corpus reduction must minimize measured encoding-behavior zones, retain split isolation, and report coverage/regret and any minimum-size bound; visual diversity clusters are not substitutes.

### AOM intra-edge continuation — corrected measured result

`ZenEnhancement::AomIntraEdgeFilter` (`aom-intra-edge-filter-v1`) is wired through the pipeline, public AVIF wrapper and static comparator at native −1, all-intra 420. It remains off by default. The comparator records the explicit pristine/hybrid reference and enhancement; serialization replay and refusal paths are checked. This is an experiment on a named reference, not strict `SvtParity`.

The expanded off/on geometry matrix exposed a real reconstruction defect: UV edge-filter strength read a luma-only 4x4 neighbor instead of the 8x8 group's chroma owner, and luma-only children overwrote the previous coded UV modes. The correction preserves chroma ownership and applies rounded group/tile availability. No angular modes, partition shapes or assertions were disabled. All 36 off/on cases now match independent decoder reconstruction at native8/10, QP0/20/48, odd65x67 and row/column tile boundaries.

The corrected canonical pilot completed **120 encodes / 40 cells / three interleaved rounds**, all deterministic and independently decoded. **All 20 SVT cells additionally reproduce the exact measured OBU and every reconstructed sample in an untimed replay**. QP20, pristine mainline base:

| Source | Setting | Bytes | bpp | SSIMULACRA2 | Median ms |
|---|---|---:|---:|---:|---:|
| Train photo1000,512×384 | Native −1 | 27,489 | 1.1185 | 71.084 | 3,187.2 |
| Same | + intra-edge | 27,384 | 1.1143 | 70.655 | 3,351.3 |
| Train screenshot8100,512×320 | Native −1 | 14,327 | 0.6996 | 84.057 | 6,181.0 |
| Same | + intra-edge | 14,344 | 0.7004 | 83.677 | 6,256.0 |

Bracketed QP20..32 estimates: at photo SSIM2=70, native is **26,333B/3,236ms** vs enhanced **26,701B/3,388ms**; at screenshot SSIM2=80, native is **11,434B/6,490ms** vs enhanced **11,629B/6,557ms**. The photo's 3.5s budget favors native −1 among these arms. At the screenshot's 4s budget neither SVT−1 arm fits, while libaom0 estimates **11,259B/3,565ms** and zenav1-aom0 **12,705B/2,783ms**. Faster SVT/AOM presets are absent from this isolated ablation, so this is not a full routing frontier. The tiny photo QP5/12 gains need broader evidence; the screenshot loses quality at every measured quantizer.

The earlier pre-fix photo gain is superseded and retained for diagnosis. The corrected experiment does not justify an automatic default. Two training origins are not the minimum representative set; no held-out calibration or HDR claim is made. Timing cohort: Core Ultra7 265K, serial nice19, codec/RAYON/OMP threads1, no affinity pinning.

Refreshed after the correction: **1,100/1,100 hybrid normal8, 1,100/1,100 pristine normal8, 320/320 pristine native−1 (160 each at8/10-bit)**; zero mismatches, pinned exceptions or harness errors in these grids. Static comparator tests:6/6. Main fetch is unchanged; all changes remain local and CI is deferred. Old real-image parity gaps and the full policy/routing/corpus objective remain open.

Local report: `zenmetrics/benchmarks/av1_compare_2026-09-08/IMAZEN26_INTRA_EDGE.md`, with complete measured cells, quality brackets and time-budget tables. Durable LAN-store archive (download SHA256 verified):

- `s3://zentrain/benchmarks/av1-compare/2026-09-08/intra-edge-corrected/evidence-3a644bc6f4c30fc3da0d3342632cc7a1325ea7695537965159d5ea75dd9197e4.tar.gz`
- SHA256 `3a644bc6f4c30fc3da0d3342632cc7a1325ea7695537965159d5ea75dd9197e4`; 709,384,601 bytes. Includes raw parity/measurement outputs, original canonical inputs, exact measured-source snapshots, pinned C sources, and measured/verifier binaries.

## Restoration-unit continuation and verified fleet measurements (2026-09-08)

Local implementation `a7f31485` adds `ZenEnhancement::AomRestorationUnitSearch`
(`aom-restoration-unit-search-v1`) immediately beyond native -1. It evaluates
legal descending unit sizes with SVT filter/rate/distortion costs and frame
signaling costs, and reaches syntax/reconstruction plus the public AVIF wrapper.
Default behavior remains C's fixed 256. This is not yet a calibrated effort mode.

The canonical zenfleet/Nomad ablation completed **144/144 encodes**, 48 deterministic
cells and **24/24 exact SVT reconstruction verifications**. A later replay
reproduced every measured input/OBU and recorded actual restoration decisions.
Both workers drained. All 30 overlapping native SVT/libaom/Rust AOM control
cells reproduced the earlier corrected run's exact outputs across CPUs/builds;
their timings remain separate cohorts.

Direct QP20 results, three interleaved timing rounds:

| Source / CPU | Arm | Bytes | bpp | SSIMULACRA2 | Median ms |
|---|---|---:|---:|---:|---:|
| Canonical photo1000 / Ryzen 3500 | SVT -1 native | 27,489 | 1.1185 | 71.084 | 6235.6 |
| Canonical photo1000 / Ryzen 3500 | SVT -1 + unit search | 27,489 | 1.1185 | 71.084 | 6393.7 |
| Canonical screen8100 / Ryzen 7900X | SVT -1 native | 14,327 | 0.6996 | 84.057 | 7236.9 |
| Canonical screen8100 / Ryzen 7900X | SVT -1 + unit search | 14,327 | 0.6996 | 84.057 | 7240.2 |

The photo selected128 only at QP5/12: +14/+5 bytes, -0.0010/-0.0652 SSIM2,
and +4.28%/+3.60% time. Other photo quantizers retained256. Screenshot
restoration search was bypassed at every tested quantizer, so identical
screenshot outputs are not an active smaller-unit RD result. Keep the
experiment opt-in; do not infer a universal failure or enable it automatically.

Bracketed matched-quality estimates (QP20..32; no extrapolation): photo at
SSIM2 70 is native SVT **26,333 B / 6335 ms** versus libaom **26,808 B / 5518 ms**.
Screenshot at SSIM2 80 is native SVT **11,434 B / 7586 ms**, libaom
**11,259 B / 3803 ms**, Rust AOM **12,705 B / 3139 ms**. Only SVT -1 and AOM0
are present here; faster-preset crossover verification remains necessary.

Local gates: **2627/2627 workspace**, **136/136 regressions**, **1100/1100 hybrid
normal8**, **320/320 pristine research** (160 each native8/10), ten enabled
geometry/depth cases, public-wrapper equality/refusal, and static comparator
library/binary checks. The comparator, worker and controller are now verified
fully static PIE. Measurement jobs automatically verify reconstruction after
timing; failures preserve evidence through zenfleet's content-addressed local/S3
stores. The S3 failure path was downloaded and SHA-verified. New source validation
also refuses high-depth inputs or nonopaque alpha in this SDR-only measurement
path; native HDR/alpha scoring remains open.

Report: `zenmetrics/benchmarks/av1_compare_2026-09-08/IMAZEN26_RESTORATION_UNITS.md`
with full cells, matched brackets, time budgets and provenance. Durable artifacts:
- Executor + canonical inputs: `s3://zentrain/benchmarks/av1-compare/2026-09-08/restoration-unit/executor-19bcb1764a4f445ad568f47c9eac3c5fd7f8bdfef58c135598f5cb0e6c9dc619.tar.gz`
- Exact source + gates: `s3://zentrain/benchmarks/av1-compare/2026-09-08/restoration-unit/source-and-gates-69c565c180832cbf90a2ef6f86389d87bde984770a7761b748bad3e7f53212e8.tar.gz`
- Analysis + exact replay: `s3://zentrain/benchmarks/av1-compare/2026-09-08/restoration-unit/analysis-and-replay-baa8d1ef31aa49aa82f24958954e1333e434334286e79127ec1ca312ca8ddede.tar.gz`
- Canonical ledger/output blobs: `s3://zentrain/jobs/av1-restoration-unit-ablation-20260908/`

These two training origins are not the minimum RD/RD-speed representative set.
Full-corpus scouting/held-out validation, strict policy/continuous effort,
actual backend routing, native HDR/format coverage, older real-image parity
regressions and main integration remain open. No CI or main push was started.

