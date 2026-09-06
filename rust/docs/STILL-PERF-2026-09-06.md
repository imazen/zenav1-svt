# Still-image performance and SIMD API audit, 2026-09-06

The current task is to bring real-image still encoding within **1.50 times
C's time**, with matched tuning, byte-identical output, baseline target CPU
and runtime SIMD dispatch. It also includes installing and exercising the
profiling tools, evaluating Archmage PRs #96 and #97 for runtime and compile
cost, and identifying any additional operations the encoder actually needs.
This task is in progress. Entries below follow experiment order; the newest
position is in "Matched PGO after large Hadamard composition" and still misses the target.


Earlier sections are preserved below as links; the current experiments follow.

## Source and machine

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#source-and-machine).

## Timing correction

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#timing-correction).

## Initial measured position

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#initial-measured-position).

## Optimization-level experiment and noise control

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#optimization-level-experiment-and-noise-control).

## Profile localization

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#profile-localization).

## Tooling and API evidence

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#tooling-and-api-evidence).

## First encoder change: screen-detection dilation

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#first-encoder-change-screen-detection-dilation).

## Incremental compile cost of #96 and #97

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#incremental-compile-cost-of-96-and-97).

## Validation and next measurements

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#validation-and-next-measurements).

## PD0 quantization reuse (next change)

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#pd0-quantization-reuse-next-change).

## PD0 coefficient energy and distortion reuse

See [the preserved experiment record](STILL-PERF-2026-09-06-history-1.md#pd0-coefficient-energy-and-distortion-reuse).

## Full-width SSE composition experiment

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#full-width-sse-composition-experiment).

## Precomputed quantizer rows (not adopted)

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#precomputed-quantizer-rows-not-adopted).

## Frame screen-content derivation reuse

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#frame-screen-content-derivation-reuse).

## Thin-LTO experiment (not adopted)

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#thin-lto-experiment-not-adopted).

## Zone-2 directional prediction: split edge runs

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#zone-2-directional-prediction-split-edge-runs).

## Narrower zone-2 arithmetic (not adopted)

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#narrower-zone-2-arithmetic-not-adopted).

## PGO with separate training images

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#pgo-with-separate-training-images).

## Position with both encoders trained

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#position-with-both-encoders-trained).

## Broader real-image position

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#broader-real-image-position).

## CLIC preset8 profile after PGO

See [the preserved experiment record](STILL-PERF-2026-09-06-history-2.md#clic-preset8-profile-after-pgo).

## V3 Hadamard 8x8 vector butterflies

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#v3-hadamard-8x8-vector-butterflies).

## DC prediction: fill contiguous output spans

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#dc-prediction-fill-contiguous-output-spans).

## Matched PGO after Hadamard and DC fills

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#matched-pgo-after-hadamard-and-dc-fills).

## EOB zero-tail batching: measured, not adopted

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#eob-zero-tail-batching-measured-not-adopted).

## Hadamard transpose: explicit safe unpacks

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#hadamard-transpose-explicit-safe-unpacks).

## Matched PGO after the unpack transpose

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#matched-pgo-after-the-unpack-transpose).

## DCT64 precision specialization: not adopted

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#dct64-precision-specialization-not-adopted).

## Coefficient absolute-sum reductions

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#coefficient-absolute-sum-reductions).

## Matched PGO after coefficient reductions

See [the preserved experiment record](STILL-PERF-2026-09-06-history-3.md#matched-pgo-after-coefficient-reductions).

## Remove the discarded film-grain estimate

Removed the unconditional full-frame heuristic whose return value was dropped.
This is not C's denoiser/model or the separately wired photon-noise table
path. No public helper was removed. User clarification requires every C
feature to be ported and wired; the audit in
[film-grain port map](film-grain-port-map.md) records the actual remaining
denoiser, supplied-table, inter-frame and recon-output gaps. They are required
missing work, not N/A. Existing historical claims of no grain signaling were
corrected without treating photon-noise support as full film-grain coverage.

All2,564 workspace tests,104 regression cases and the three photon-noise
decode checks pass. The decode checks compare pre-grain reconstruction with
`aomdec --skip-film-grain`, and require normal decoding to differ. They do not
prove C-equivalent grain-applied reconstruction output. No fresh full envelope
sweep is claimed for deleting a discarded calculation outside mode decisions.

All99 non-PGO frame A/B pairs are identical. Wiki1024/QP60/p8 improves2.31%;
NYC512/QP60/p2 improves1.22%; CLIC512/QP40/p8 improves1.01%. Several smaller
changes have overlapping/noisy intervals, including terminal p2's0.06%
regression. All99 same-binary control pairs are also identical; control medians
range0.9968–1.0058. Wiki control quartiles0.9969–1.0073 are separated from
the candidate0.9735–0.9800. NYC control and candidate spans nearly touch,
so its smaller gain is less decisive. The production
text section shrinks704 bytes. Records and raw artifact hashes:
`benchmarks/still_i265_2026-09-06-grain-drop-frame-{ab,control}.*`.
The current matched-PGO position still refers to coefficient reductions;
no fresh PGO result is claimed for this cleanup yet.


## Matched PGO after discarded-grain cleanup

Encoder source `136e779c` matches all108 C training-output hashes. The fresh
held-out sweep has675/675 identical pairs and **77/135 medians and77/135
upper quartiles at or below1.50**, versus76/76 before cleanup.58 median cases
still miss the goal. NYC512/QP60/p2 is1.8854 (previously1.8831), and
wiki1024/QP60/p8 is1.7878 (previously1.7824); these do not demonstrate a
clear PGO gain despite the non-PGO A/B improvement. Small changes and threshold
crossings remain sensitive to run variation. Wiki p6 is1.8303.

The same baseline CPU, core2, training inputs and C PGO binary are retained.
The sweep takes412s, peak monitored RSS0.07GiB, minimum available memory
29,179MiB. Records: `benchmarks/still_i265_2026-09-06-pgo-grain-drop-{training,broad}.*`.


## Held-token large Hadamard composition

The candidate dispatches once for16x16 or32x32, carries its V3 token through
the8x8 sub-transforms, and compiles the unchanged combine arithmetic under V3
features. Existing scalar/non-x86 arithmetic remains the fallback. No new
Archmage operations are required. The C AVX2 oracle is essential here: at
high-bit-depth residuals it wraps where scalar C can retain wider values.

A600-case probe checks exact coefficient positions against frozen Rust,
coefficient multisets against real C AVX2 (which permutes positions), and
output sentinels. The production test also runs600 cases per token permutation
and checks scalar-C positions on bounded patterns. It passed on the original
production body before editing. Deliberate incorrect16x16 and32x32 shifts
both fail at constant255 inputs; restored code passes2,566 workspace tests
with zero skipped and104 regression cases.

The first probe compared against the production DSP dependency. A second
probe freezes the baseline source to prevent future changes moving it. Its
600 cases pass and the candidate improves roughly13–15% across six groups.
All runs report zero gate waits and reliable timing; control biases are
recorded in the table: had16/stride32 shows+0.73% with its interval excluding
zero; the other five frozen control intervals contain zero. Text grows2,844
bytes. Frozen probe: `tools/perf_profile/hadamard_compose_probe`; records:
`benchmarks/still_i265_2026-09-06-had-compose-probe.*`.


All99 production frame A/B pairs and99 same-binary control pairs are
byte-identical. NYC512/QP60/p2 improves1.59%, with candidate quartiles
0.9836–0.9868 versus control0.9978–1.0053. Wiki's1.24% median improvement
has candidate quartiles0.9852–0.9898 overlapping control0.9860–1.0002,
so a separate wiki gain is not established. CLIC p2/p6/p8 medians improve
0.69%/0.66%/0.94%; the remaining improvements are below0.5% and several
are near control variation. Control medians span0.9927–1.0061.

Disassembly retains four direct calls from the V3 32x32 function to its
held-token16x16 helper; the optimization does not inline the entire family.
The archived frozen probe's test passes too. No full-envelope rerun is claimed
for this DSP-only change. No matched-PGO result includes this change yet.
Frame records: `benchmarks/still_i265_2026-09-06-had-compose-frame-{ab,control}.*`.


## Matched PGO after large Hadamard composition

Encoder source `9e8a28a1` matches all108 C training-output hashes. The fresh
held-out grid has675/675 identical pairs and **80/135 medians and80/135
upper quartiles at or below1.50**, versus77/77 before the change.55 median
cases still miss the goal. Wiki1024/QP60/p8 improves from1.7878 to1.7227;
wiki p6 is1.8109. NYC512/QP60/p2 instead measures1.9303, versus1.8854 in the
previous grid: Rust time rises432.19→437.34ms while C falls229.23→226.16ms.
The higher threshold count does not establish an across-the-board gain.

A follow-up15-pair PGO Rust/Rust comparison isolates the two Rust binaries.
NYC measures0.9858 (quartiles0.9821–0.9924) and wiki0.9741
(0.9681–0.9805), with all30 outputs identical. Fresh same-binary controls
are0.9993 (0.9973–1.0032) and1.0026 (0.9972–1.0172), all30 identical.
These support direct PGO gains on both cases, but do not replace or repair
individual cells in the separate Rust/C grid. Its1.9303 worst ratio remains
the reported broad-grid result. Baseline CPU, core2, separate training data
and the unchanged C PGO binary preserve the comparison protocol.

The grid takes411s, peak monitored RSS0.07GiB, minimum available memory
29,239MiB. Records: `benchmarks/still_i265_2026-09-06-pgo-had-compose-{training,broad}.*`
and `benchmarks/still_i265_2026-09-06-had-compose-pgo-frame-{ab,control}.*`.

Fresh whole-process perf captures also match outputs with zero lost samples:
NYC245,086 Rust/130,429 C samples, wiki samples in the linked metadata.
NYC self shares: zone2 split7.29%, transform-search closure6.24%, directional
wrapper6.12%, square-forward wrapper4.92%, fdct64 3.87%. The wrapper can now
absorb functions previously reported separately, so self-share changes alone
are not speed changes. Wiki still has square-forward wrapper11.31%, fdct64
10.50%, memset6.26%, held-token Hadamard16 5.38%, inverse64 4.36%.
Both include construction, teardown and logging; internal frame clocks remain
the performance measurements. Tables and artifact hashes:
`benchmarks/still_i265_2026-09-06-had-compose-{nyc,wiki}-*-profile.tsv` and
`benchmarks/still_i265_2026-09-06-had-compose-{nyc,wiki}-profile.meta.json`.
