# Goal: simple effort, strict parity, HDR and adaptive AVIF routing

User-directed workstream, 2026-09-08. This document defines work to complete;
its remaining criteria must be read alongside the implementation checkpoint below. It complements the separate animated-AVIF/video objective rather than
declaring that objective complete.

## Outcome

Ship a simple encoding interface in zenav1-svt and zenavif built around
**quality, continuous effort and policy**. Preserve useful HDR-fork and Rust
extensions, fully support C's research preset -1, provide strict `SvtParity`,
and implement measured content-adaptive search and actual backend routing.
Complete the experiments and report size, perceptual quality and time before
choosing defaults. Land the ready implementation after required local checks.

## Current handoff checkpoint — 2026-09-08

Policy/reference/−1 APIs, checked bucketed effort, support queries, actual
zenavif routing, Gray8, serialized replay/cache identity and film-grain wiring
are on main. SVT implementation `0cbd1279`; zenavif API landing `dba8f5ee`.
Read [CONTEXT-HANDOFF.md](CONTEXT-HANDOFF.md) for observed current consumer/AOM
pins and the remaining ordered work. Earlier “local/not merged” checkpoints
are [archived](rust/docs/history/2026-09-08/ENCODER-POLICY-GOAL.md).

The full goal remains incomplete: four native10 cells, fractional adaptive
budgets, broad imazen-26/held-out calibration, backend-owned measured optimality,
remaining C video/HDR/superres wiring and useful alternate-backend formats.
Requirements below define the final target; already landed portions must not
be reimplemented just because the original checklist uses future tense.

## 1. Establish the actual feature and reference inventory

- Reconcile current main, local work, open issues and the existing C/HDR port
  maps. Trace settings through configuration, derivation, search, coding,
  reconstruction, metadata and wrappers. A helper with no caller is not support.
- Distinguish shipping mainline-C features, hybrid/HDR-fork features, useful
  Rust extensions, unsupported formats and proposed experiments. Preserve
  existing useful extensions; do not remove them to simplify parity.
- Pin mainline and HDR/hybrid reference sources, build modes and effective
  settings. Revalidate historical parity claims. The HDR fork includes changes
  beyond optional knobs; neutral knob values alone do not establish mainline
  parity. Audit unconditional differences and the actual `SVT_HDR_MODE` split.
- Reconcile the earlier 106-regression report and all subsequent real-image
  parity witnesses. Reproduce C-shared behavior in parity mode; fix Rust-only
  translation/wiring defects without loosening expectations or disabling tools.

## 2. Make the API small, explicit and reproducible

Separate **policy**, **effort**, **native preset** and **content requirements**.
The ordinary user controls quality, effort and policy; image format/metadata
remain explicit requirements. Preserve existing integer-preset entry points
with documented compatibility behavior.

- `SvtParity` targets an explicitly identified, pinned C implementation. The
  default target is mainline SVT. If HDR/hybrid parity is offered, its reference
  target must be named explicitly and have its own gates. Never silently switch
  which C implementation is meant by parity.
- In `SvtParity`, reject unsupported formats/settings and incompatible Zen
  overrides. Use only reference-supported settings and reference-equivalent
  decisions. Do not route to AOM or silently substitute a Rust extension.
- A Zen policy enables the calibrated adaptive behavior and appropriate
  existing HDR-fork features. Keep independently selectable enhancement enums
  or a set for diagnostics and ablation, rather than requiring ordinary users
  to choose every combination. Add validated bundles only when evidence supports
  them; never make an unmeasured experimental bundle the default.
- Introduce a checked `Effort` float with a documented bounded scale; higher
  effort means more permitted work. Reject NaN, infinities and invalid ranges.
  Native research preset -1 is a separate concept, not negative effort.
- Fractional effort resolves to an actual bounded search policy: content-aware
  candidate budgets, refinement depth, tool evaluation and similar measured
  choices. Do not linearly interpolate preset identifiers or imply the C encoder
  has fractional presets. In parity mode, require or deterministically resolve
  to a supported native C preset and report that resolution; no Zen adaptation.
- Record a versioned resolved configuration/decision policy, reference identity,
  backend, native preset, enhancements and reason for adaptive choices. Include
  these identities in benchmark rows and encode/cache fingerprints. Provide
  deterministic replay and stable serialization; no unrecorded timing-driven
  or randomized decisions. Treat a calibrated time budget as an estimate, not
  a portable exact deadline.

## 3. Fully port and wire research preset -1

- Add a typed research preset or checked signed representation and carry it
  through every affected derivation. Audit unsigned comparisons, clamps,
  array indices, preset ladders and wrapper mappings.
- Port all live research-mode differences, including restoration, CDEF and
  mode/partition/transform search, their rate costs, syntax and reconstruction.
  Merely accepting -1, casting it to `u8`, or aliasing preset 0 does not qualify.
- Keep -2/-3 rejected for reference builds whose public validator rejects them;
  an enum name in C is not proof of supported behavior.
- Prove enabled end-to-end parity with the pinned C -1 reference across useful
  quality levels, content classes, native 8/10-bit input, boundaries and tile
  settings. Cover wrapper/query reachability as well as the raw pipeline.

### Next effort region: Zen continuation below native -1

Immediately after completing native -1, prototype and port the useful AOM
techniques into SVT itself. This is the next work item and the next region on
the slow-preset ladder: **C -1 is the parity anchor; below -1 is Zen-enhanced
search**. It must not be satisfied merely by routing the request to AOM.

If the API exposes a preset-style fractional coordinate, values below -1
identify this Zen continuation; they are not C presets. Keep the policy and
native reference preset separate in the resolved configuration so these values
cannot collide with C's named-but-rejected -2/-3 enum entries. On an increasing
effort scale, the same region sits above the effort assigned to native -1.
`SvtParity` rejects the Zen-only region rather than silently claiming C parity.

Start with isolated restoration-unit/SGR/intra-edge and search-pruning
experiments, then justified combinations. Measure improvements against both
native SVT -1 and AOM's matched-time frontier. The region can combine broader
search with better pruning; more effort is not a mandate to retain wasted
search. Reuse successful techniques at lower efforts only when separate
measurements support it. Retain the explicit -1 anchor and explain measured
failures rather than forcing every candidate into a shipping preset.

## 4. Integrate HDR-fork features and adaptive candidates carefully

- Inventory and verify the existing HDR-fork controls and their effects in the
  Rust encode path. Include perceptual/lambda and chroma tuning, variance/QP
  behavior, transform biases, noise/grain handling, filtering and metadata where
  the actual source supports them. Do not assume a feature applies only to HDR
  because of the fork's name, or that all fork features improve all images.
- Preserve transfer functions, primaries, range, native precision, HDR metadata
  and alpha through wrappers and backend selection. Use real HDR references and
  appropriate HDR scoring for HDR claims; RGB8 converted to 10/12-bit is an SDR
  precision experiment, not an HDR corpus.
- Verify the screen-content detector's real results, preset gates, force modes
  and downstream palette/IntraBC decisions. Expose reusable content evidence
  where appropriate and validate mixed photo/text cases. Preserve C's exact
  detection and tool policy in `SvtParity`.
- Evaluate the documented AOM adoption candidates: restoration-unit sizing,
  still SGR, gradient-based directional pruning, learned transform-depth pruning
  and intra-edge policy. Distinguish tools that already exist but are gated from
  missing machinery. Attribute gains with isolated ablations, not source presence.
- Validate or retrain learned pruning against SVT's own costs and exhaustive
  winners; do not assume AOM's weights and thresholds transfer.

Control combinatorial growth deliberately: measure individual effects first,
then interactions justified by those results. Code stable universal choices;
derive predictable choices from content/effort; reserve runtime selection for
choices with demonstrated content dependence. Use bounded experiments rather
than crossing every knob, preset, format and image. Keep the complete feature
inventory even when the calibration grid is deliberately sampled.

## 5. Complete corpus-backed calibration and fleet experiments

- Obtain and verify the canonical **imazen-26** repository/manifest. Use the
  prescribed origin-level train/validation/test split and registered variants;
  derivatives must stay in their source's split. Do not substitute an unverified
  cache, invent a split or fit the policy to held-out test images.
- Reduce expensive repeated experiments to the **smallest demonstrated set of
  encoding-behavior representatives**, using the procedure below. Select based
  on measured RD and RD-speed zones unique to images, not visual similarity,
  image embeddings, arbitrary class quotas or a convenient fixed sample count.
- Cover photographic, screenshot/text and mixed content, useful image sizes,
  8/10/12-bit and chroma formats where supported, high-quality/lossless/alpha,
  representative tuning/SCM and threading. Add a properly identified native HDR
  corpus. Unsupported cells must be declared explicitly, not silently skipped.
- Extend the static native-API comparison for C SVT, zenav1-svt, libaom,
  zenav1-aom and zenrav1e. Compare identical input representations and clearly
  separate conversion/encoding/container overhead. Preserve source/configuration,
  code/build and binary provenance, outputs and effective settings.
- Dispatch distributed work through canonical zenfleet declaration, capability,
  claiming, retry, ledger and artifact mechanisms. Keep matched comparisons on
  the same worker, isolate timing resources, monitor progress and drain workers.
  Complete and collect the jobs; a declared manifest is not a completed sweep.
- Use at least three interleaved timing rounds for reported cells, independent
  decoding and appropriate perceptual metrics. Report bytes/bpp, achieved quality
  and encoding time together. Include all normal SVT efforts, research -1,
  fractional/adaptive regions and dense quantizer brackets near policy boundaries.
- Compare rate-distortion under common time budgets, not equal preset numbers.
  Do not pool different CPUs' timings, extrapolate quality or hide reversals.
  Treat sparse-grid interpolation as an estimate and directly verify selected
  crossover settings. Report held-out aggregate results, content breakdowns and
  regressions, not just a winning example or average.

### Derive the minimum representative encoding set

1. Run a bounded scouting sweep across canonical imazen-26, using verified
   compatible historical rows where possible. Include backend/effort anchors,
   native SVT -1, useful quality ranges and relevant format/HDR-feature strata.
   Refine only uncertain curves and crossover regions; the initial scout need
   not be the full dense Cartesian grid. Never label unmeasured regions covered.
2. Build per-origin behavior signatures from **rate-distortion curves and
   rate-distortion-time surfaces**: best backend/configuration under quality and
   time constraints, winner changes and crossover locations, marginal size gain
   per added effort, saturation/floors, format sensitivity and measured response
   to HDR-fork/adaptive tools. Retain bytes/bpp and absolute latency as well as
   size-normalized descriptors; normalization must not erase small-image startup
   effects or other actual routing distinctions. Keep hardware cohorts separate.
3. Define behavior zones over quality, effort/time, backend and relevant format
   strata. Preserve an image if it contributes a distinct winner region, RD
   shape, crossover, tool response or other material encoding behavior that the
   current representatives cannot explain. Keep correctness failures and rare
   regression witnesses in a separate mandatory suite even when performance
   redundancy would otherwise remove them.
4. State equivalence and coverage tolerances before selection, based on timing
   uncertainty and an explicit acceptable RD/routing-regret budget. Do not relax
   these tolerances after seeing the desired subset size. Confirm apparent unique
   zones with repeat measurements so noise does not manufacture representatives.
5. Minimize the number of selected origins subject to covering every established
   behavior zone within those tolerances. Use set-cover/medoid selection followed
   by removal and swap checks; use exact optimization where tractable. Report
   the achieved count, coverage, irredundancy and any optimality bound/gap.
   Do not call a greedy result the proven global minimum. Every retained origin
   must have a documented reason it cannot currently be removed.
6. Preserve canonical split isolation. Fit selection rules, adaptive policies
   and learned pruning using training origins; validate on validation origins.
   Keep test evaluation separate and untouched by policy selection. Any compact
   evaluation panels must retain their split labels and cannot become training
   data. Corpus-wide scouting must not leak held-out outcomes into policy fitting.
7. Validate the reduced training set and resulting policy against the full
   prescribed validation/test populations. Report worst-case and percentile
   size/quality/time regret, missed zones and changed backend choices, not only
   means. An image outside the current model's coverage triggers targeted
   refinement and a new version of the representative manifest; do not silently
   waive it. Do not tune against test failures and continue calling that test
   set untouched; record exposure and use an appropriately independent gate.
8. Persist source hashes, splits, representative assignments, zone signatures,
   supporting measured rows, selection parameters and reasons for retention or
   exclusion. Version the subset against codec/build and policy identities.
   Re-scout when new HDR/AOM techniques or encoder changes create new behavior;
   a fixed subset is not permanently representative of a changing encoder.

Use this compact set for the dense iteration loop, with periodic full-corpus
checks and final held-out evaluation. The deliverable includes the measured
reduction in corpus size and experiment cost, together with its coverage and
regret evidence; reducing image count alone is not success.

## 6. Wire real capability-based zenavif routing

- Have zenav1-svt and zenav1-aom own queries for supported input/output features
  and versioned speed/RD suitability at the requested quality/effort/policy.
  Evaluate zenrav1e as an alternate using the same evidence. Keep capability and
  measured preference distinct; an unsupported capability cannot win a ranking.
- Implement the selection and execution in zenavif, not merely a query module.
  Route reasonable still needs outside SVT's envelope to a suitable backend;
  route supported cases elsewhere when held-out speed/RD evidence justifies it.
- Preserve useful SVT extensions and all required precision, color/HDR metadata,
  alpha and lossless semantics. Respect explicit backend and parity requests.
  Return a clear error where no backend can satisfy the request; do not silently
  degrade the requested format or quality semantics.
- Make the resolution inspectable and reproducible. Test reported support
  against actual encodes, including refusal paths, routing boundaries and
  metadata/alpha round trips. Avoid duplicating a drifting support table inside
  zenavif when the backends can report it themselves.

## Completion gates

The goal is complete only when all of the following are true:

1. Public quality/effort/policy APIs and the native research preset are usable
   through the real wrappers, with documentation, validated bounds and replay.
2. `SvtParity` enforces its pinned C envelope, and all required parity/conformance
   witnesses in the delivered scope pass without weakened gates. Research and
   HDR-enabled behavior have enabled-feature tests, not only neutral/off tests.
3. The selected adaptive enhancements are implemented, wired, independently
   decoded and justified by completed ablations and held-out corpus results.
   AOM-technique adoption has been attempted in the Zen continuation immediately
   beyond native -1, with measured results against that anchor and AOM.
   Candidates that fail to help are documented and excluded from automatic
   policy without stripping existing features or concealing coverage gaps.
4. Actual zenavif routing consumes backend-owned support/suitability, respects
   user requirements, and passes format/metadata/alpha and selection tests.
5. Completed local/fleet results, raw encoded artifacts, grids, provenance and
   analysis are durably stored. The user receives size/quality/time results and
   the measured routing/effort decisions, including limitations and regressions.
   The versioned reduced imazen-26 set documents each representative's unique
   encoding zones, achieved minimum/optimality status, full-population validation
   and measured compute savings.
6. The tracking issue, SVT-vs-Rust/backend feature matrix and context handoff
   reflect verified final behavior and any explicitly separate remaining work.
7. Required local tests, parity gates, format/lint checks and relevant integration
   checks pass before CI. Reconcile current main, push and merge the ready work
   under the user's existing authorization, then verify it is reachable on main.
   Do not claim completion based on an unpushed change or a passing smoke test.

Existing evidence and starting points:

- [Completed comparison and time-budget curves](../zenmetrics/benchmarks/av1_compare_2026-09-08/README.md)
- [AOM adoption audit](../zenmetrics/benchmarks/av1_compare_2026-09-08/AOM_ADOPTION.md)
- [HDR hybrid history and oracle caveats](rust/docs/HDR-ON-4.2.md)
- [Current work index](CONTEXT-HANDOFF.md)
- [Remaining-gaps tracking issue](https://github.com/imazen/zenav1-svt/issues/21)
