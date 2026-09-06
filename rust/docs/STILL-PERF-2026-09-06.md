# Still-image performance and SIMD API audit, 2026-09-06

The current task is to bring real-image still encoding within **1.50 times
C's time**, with matched tuning, byte-identical output, baseline target CPU
and runtime SIMD dispatch. It also includes installing and exercising the
profiling tools, evaluating Archmage PRs #96 and #97 for runtime and compile
cost, and identifying any additional operations the encoder actually needs.
This task is in progress. Entries below follow experiment order; the newest
position is in "Matched PGO after Hadamard and DC fills" and still misses the target.

## Source and machine

The checkout was rebased onto `origin/main` at `4285249e`. Existing setup work
was preserved as jj change `mqvxnlpk`: `AGENTS.md` and a Cargo.lock recording
local Archmage dependencies. The ignored `.cargo/config.toml` redirects all
three Archmage packages to `/home/lilith/work/archmage`, whose HEAD was verified
as `2524b0be162ca04ceb9a39906e19fe9c9255657e`, clean. This is a machine-local
override, distinct from the manifest's `cc24398c` Git pin.

Host: `i265`, Intel Core Ultra 7 265K, native Linux 7.0.0-30, 30 GiB RAM.
The kernel's `cpu_core/cpus` is `0-7`; `cpu_atom/cpus` is `8-19`. Measurements
pin both encoders to performance core 2. Background zenfleet/zenmetrics work
is present; this is not an idle-host claim. Both timed arms inherit nice 19
and idle I/O priority from the shared run-heavy wrapper, with a 16G cgroup cap.
The governor is `powersave`. Same-binary controls are recorded below.

Rust: 1.98.1, LLVM 22.1.8. The initial build uses the workspace release profile
(opt-level 2 for workspace crates), with RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS
unset. C's CMake cache records Release `-O3 -DNDEBUG`, `NATIVE=OFF`,
`ENABLE_AVX512=ON`, `SVT_AV1_LTO=OFF`. This host cannot execute AVX-512.
The C submodule HEAD matches the outer pin, `3115c0c1b23e860dfd75c94f6740e0298182dd13`.

## Timing correction

Source inspection found that `tools/perf_c_encode/perf_c_encode.c` called
`fopen`/`fwrite` between its two clock reads; the Rust driver writes output
after its clock stops. The C driver now copies drained packets into owned
memory and releases the encoder buffers inside the timed region, then writes
files outside it. Warmups perform the same copies. Per-PTS output is retained.
This changes the measurement method, not encoder speed. Old timings are not
an interchangeable baseline. The saved old driver and corrected driver produced identical one-frame and
2-frame outputs, including both per-PTS files. A delayed FIFO reader blocked
output opening for two seconds: the old driver's internal time was 1998.750 ms,
while the corrected driver reported 17.488 ms despite the same two-second
process wall time. This directly demonstrates exclusion of output I/O.
The runner also rejected `/bin/true` for missing timing output. Record:
`benchmarks/still_i265_2026-09-06-timing-validation.txt`.

`tools/perf_profile/still_pairs.py` uses prebuilt binaries, randomized paired
order, the drivers' internal API timings, and one warmup per sample. It checks
the bytes on **every sample**, rejects missing/invalid timings and process
failures, and records executable/input hashes and CPU affinity. The manifest
gives an explicit I420 input for each size; it never reinterprets the prefix
of a larger raw image as a smaller image.

## Initial measured position

Each row is nine pairs, 512x512, QP 40, 8-bit 4:2:0 still/AVIF. Every sample
is byte-identical. Ratios are medians of per-pair port/C times, not ratios of
independently collected blocks. These three images are an initial localization
set, not evidence across sizes, qualities, bit depths or the full corpus.

| Image | Preset 2 | Preset 6 | Preset 8 |
|---|---:|---:|---:|
| CID22 training/3571065.png | 1.4093 | 1.6674 | 1.7002 |
| CLIC2025 training/ef576c4ed599d75d72145a8f34b58ccb.png | 1.7588 | 1.9022 | 1.7587 |
| gb82-sc/terminal.png | 1.6366 | 1.5909 | 1.7136 |

Inputs were produced by the existing `identity_run crop:` path. CID22's I420
SHA256 is `88142a484affd4b36c36ce56ad4e1782811235958b38078e9aa0704d3ca4a72f`,
matching the earlier campaign's source. Full samples, binary hashes and input
hashes are in `benchmarks/still_i265_2026-09-06-opt2.{tsv,raw.tsv,meta.json}`.
Wrapper report: 104s, peak RSS 0.04 GiB, minimum available RAM 26041 MiB,
peak system load 3.16. These are wrapper resource observations, not a new
encoder memory comparison.

## Optimization-level experiment and noise control

A separate opt-level-3 build retained baseline target CPU and the same runtime
SIMD dispatch. Nine pairs per cell remained byte-identical:

| Image | Preset 2 | Preset 6 | Preset 8 |
|---|---:|---:|---:|
| CID22 | 1.3942 | 1.6348 | 1.7050 |
| CLIC2025 | 1.7295 | 1.8284 | 1.7760 |
| terminal | 1.6164 | 1.5747 | 1.7030 |

Record: `benchmarks/still_i265_2026-09-06-opt3.{tsv,raw.tsv,meta.json}`.
This remains eight failing cells. Separate positions do not substitute for a
direct paired opt2/opt3 A/B experiment. No release-profile change was adopted.

Twenty-one same-binary opt2 pairs on the three preset-6 cells gave median
ratios 1.0024, 1.0009 and 0.9994 respectively. All interquartile intervals
include 1.0; the overall interval endpoints range from 0.9969 to 1.0052.
Every pair was byte-identical. Record:
`benchmarks/still_i265_2026-09-06-control.{tsv,raw.tsv,meta.json}`.

## Profile localization

`perf record -e cycles:u -c 100000 --call-graph dwarf,8192` captured the CLIC
512x512 QP40 preset6 opt3 and C drivers, pinned to core2, each with 60 warmup
cycles and one reported encode. Outputs were byte-identical. The profiles
include constructors and teardown, so neither total sample counts nor these
shares are encode-only time comparisons. In the port's cpu_core self samples,
`optimize_b` is 6.65%, `tx_unit_inner` 4.91%, `dilate_block` 3.60%,
`sc_aa_collect_counts` 3.50%, and the current generic SSE kernel 1.12%.
The gap is distributed; the new integer API's sole current consumer cannot
close it alone. Saved profiles and disassembly are under
`~/tmp/still-perf-2026-09-06/`. Disable debuginfod (`DEBUGINFOD_URLS=`) when
reporting to avoid external symbol-download stalls. These large diagnostic
files are local scratch, not a durable profile archive.

## Tooling and API evidence

Installed and version-checked: cargo-read 0.1.0, cargo-show-asm 0.2.62,
cargo-llvm-lines 0.4.48, cargo-bloat 0.12.1, flamegraph 0.6.14, samply 0.13.1.
`perf`, valgrind and heaptrack were already present. A real perf-stat probe
initially failed at `perf_event_paranoid=4`; setting it to 1 made user-mode
hardware counters work. This sysctl change is runtime-only. cargo-show-asm was rebuilt with its optional `disasm` feature and successfully
inspected the saved encoder's `sc_detect::dilate_block` without rebuilding it.
Intel's official APT repository supplied VTune 2026.4.0 (build 632893), and
Ubuntu supplied LLVM tools 22.1.2, including llvm-mca. These command versions
were exercised. The VTune hardware collection ran the target but aborted with
`std::bad_alloc` (exit 134); its software collector refused the host's ptrace
restriction (exit 1). Installation is complete, collector usability is not.
`perf` remains the working hardware profiler.

The requested `cargo read magetypes` and `cargo read archmage` both completed
and fetched published 0.9.28. Their documentation and the local checkout's
source were read alongside `claudehints/topics/rust-defaults.md` and
`topics/benchmarking.md`. Retain fixed-array load/store contracts, put dispatch
outside hot loops, and inspect complete compositions, including widening and
reduction. A token being zero-sized does not prove an entire kernel is optimal.

Live GitHub state, checked this session:

* [PR #96](https://github.com/imazen/archmage/pull/96) is OPEN, head
  `9981d4f9ad90b0cc57bb244e1b1c16df761bdb20`, base
  `1bfc3c5b1618ddddd23d0e85d0c9ab8583d660b2`.
* [PR #97](https://github.com/imazen/archmage/pull/97) is OPEN, head
  `43f8baed6e859b393ff7f42dfad7a40f49464ee9`, based on #96's current head.

Both heads are newer than the local dependency. PR descriptions report
primitive/complete-chain intrinsic comparisons, but also retain unfused x86
SAD and WASM widening differences. The current #97 source was downloaded separately without changing the
load-bearing sibling checkout. This session ran its integer codegen checker:
x86 36/41, NEON 23/23 and WASM 23/25 comparisons matched direct references.
All seven differences are the explicitly reported alternative compositions:
five unfused x86 SADs and two WASM load-extend chains. The ten same-load
byte-to-signed-dot chains match. These are compiler-output comparisons,
including cross-compilation, not measurements of ARM/WASM runtime speed.
Native `int_widen_narrow` tests passed 5/5 (scalar and V3 included; AVX-512
hardware is absent). Record: `benchmarks/still_i265_2026-09-06-pr97-codegen.txt`.

Disassembly of the initial opt-level-3 experiment confirms that the encoder's
current 16-byte SSE fold still uses 128-bit min/max/subtract, two pairwise
multiplications and two accumulator additions. There are no calls between
these arithmetic instructions. The later full-width SSE composition experiment below measures that alternative.
Incremental compile costs of the current PR heads are recorded below.
Do not attribute the earlier scalar-to-SIMD speedup to abstraction overhead
removal: that experiment changed the algorithm's arithmetic width and packing.

A follow-up CLIC preset8 candidate profile explicitly selects
`cpu_core/cycles/u`, because flamegraph's default selection from the original
hybrid-PMU recording picked only the five taskset startup samples from
cpu_atom. That first SVG was rejected after finding no encoder symbols.
The corrected recording/graph contains 274 encoder symbols, with 21K retained
samples and 113 lost samples. It localizes `pd0::tx_quant_core` (11.27% self)
and hadamard/transform work as the next leads. These whole-process shares
remain localization evidence, not an encode-only speed claim. Files:
`clic-port-core.perf`, `clic-port-core.svg`, `clic-p8-core-self.txt` in scratch.

## First encoder change: screen-detection dilation

The V3 path computes horizontal masks from original rows, unions each with
its two neighbouring rows, and selects the dominant pixel value. It replaces
C's eight conditional scatter stores with existing Archmage intrinsics under
`forbid(unsafe_code)`. Shapes beyond 16 rows, widths other than 8/16, and
overlapping destination strides retain the original scalar implementation.
No new dependency API was needed.

An initial 21-pair preset6 experiment found candidate/baseline ratios of
0.9988 (CID22), 0.9758 (CLIC), and 0.9869 (terminal). After adding the
overlapping-stride fallback, nine pairs across the full initial grid gave:

| Image | Preset 2 | Preset 6 | Preset 8 |
|---|---:|---:|---:|
| CID22 | 1.0023 | 0.9992 | 0.9951 |
| CLIC2025 | 0.9968 | 0.9737 | 0.9949 |
| terminal | 1.0001 | 0.9901 | 0.9985 |

All 81 final pairs were byte-identical. The clear improvement is preset6 on
CLIC (~2.6%) and terminal (~1.0%); the remaining changes are small relative to
the same-binary control variation. These are direct candidate/previous-Rust
ratios at opt-level3, not new port/C ratios. Record:
`benchmarks/still_i265_2026-09-06-dilate-ab.{tsv,raw.tsv,meta.json}`.

The C parity test now exercises dispatch permutations, tight/padded and
overlapping strides, thin rectangles and scalar-fallback shapes. An explicit
8-column dominant-zero case checks that unloaded high lanes cannot spill
into column 7. Deliberately omitting that mask failed with column 7=0 instead
of 17; the correct mask was restored. Final workspace nextest passed 2557/2557
with zero skipped, and regression spot-check passed 104/104. This is a small
measured improvement, not completion of the 1.50x goal.

## Incremental compile cost of #96 and #97

Five randomized rounds rebuilt archmage, magetypes and an identical consumer
of the pre-existing f32x8 API, with all other dependencies cached. The three
source snapshots are #96's base `1bfc3c5b`, #96 `9981d4f9`, and #97 `43f8baed`.
The consumer forbids unsafe, has a V3 token entry point, and computes
`(v * v).reduce_add()` after loading eight floats. Both configurations use
`std`; one also enables `w512`. No AVX-512 feature is enabled in this small
probe. Cargo incremental compilation is disabled, baseline target CPU is
retained, and each trial checks Cargo JSON to prove all three crates rebuilt.

Median elapsed seconds, including Cargo overhead:

| Source | check, no W512 | check, W512 | release build, no W512 | release build, W512 |
|---|---:|---:|---:|---:|
| Before #96 | 0.750 | 0.973 | 0.881 | 1.179 |
| #96 | 0.745 | 0.985 | 0.892 | 1.186 |
| #97 | 0.765 | 0.986 | 0.890 | 1.199 |

The added APIs do not show a large marginal compilation penalty in this
probe: median changes are roughly -5 to +20 ms. Enabling the existing W512
surface has a substantially larger cost, roughly 0.22-0.31 s. This is neither
a cold workspace build nor a measurement of compiling the new consumers;
it does not justify removing the encoder's intentional AVX-512 default.
Records: `benchmarks/still_i265_2026-09-06-compile-cost.{raw.tsv,meta.json}`.
The driver is preserved at `tools/perf_profile/compile_cost.py`; its input
and output roots are explicit arguments, while the recorded experiment
used the fixed scratch paths in its metadata.

An initial invalid run used `cargo clean -p` without `--release`, removed
zero files, and measured cache hits. It was discarded. The corrected run uses
`cargo clean --release -p archmage -p magetypes -p compile_cost_probe` and
rejects any sample whose compiler-artifact messages report those crates fresh.

Current API assessment: the casts in #97 add no observed conversion cost to
the tested chains; #96's primitive and fused operations match their explicit
intrinsic references. Neither PR alone demonstrates an encoder speedup.
The initial SSE consumer's split 128-bit arithmetic was subsequently
compared with a full-width kernel in the SSE composition experiment below. Keep specialized raw-intrinsic paths where a
complete composition is worse; these findings do not warrant adding further
Archmage methods for this encoder yet.

## Validation and next measurements

On the synced source with the original local dependency: workspace nextest
**2557/2557**, zero skipped; regression spot-check **104/104**. Logs are in
`~/tmp/still-perf-2026-09-06/`. Existing warnings include private-interface
visibility and C static symbols unavailable for promotion. A formatting check
found pre-existing reflow differences in an entropy-context test; no source was
discarded or reformatted during this measurement.

Next: collect encode-focused profiles across the remaining cells; broaden
size/quality/content coverage and optimize measured hot paths until the 1.50x
goal is demonstrated. The goal remains active.

Before landing the dilation change, the machine-local dependency override was
removed temporarily and both gates were rerun against the manifest's tracked
`cc24398c` Git pin with its tracked lockfile: `cargo nextest run --locked
--workspace -j 4` passed 2557/2557, zero skipped; regression spot-check
passed 104/104. The local setup change was kept separately from the changes
being shipped. Performance artifacts above retain their explicitly recorded
local dependency and opt-level3 provenance.

## PD0 quantization reuse (next change)

A CLIC preset8 profile attributed 11.27% self samples to `pd0::tx_quant_core`.
Its non-QM quantizer duplicated the scalar scan-order loop, despite the coding
path already using `quant_coding::quantize_b_raster`. PD0 now reuses that
existing kernel and the existing reverse-scan EOB helper. Its original loop
is retained as a test-only reference; the QM path is unchanged.

The new parity test sweeps all 256 qindices, 11 relevant DCT scan shapes,
all-zero blocks, signed dead-zone boundaries with a zero suffix, clamp
boundaries and a last-coefficient-only block, under dispatch permutations.
It checks both the former body and real scalar/dispatched C. A deliberate
EOB-minus-one mutation failed and was restored. Workspace nextest passed
2558/2558, zero skipped; regression spot-check passed 104/104.

Twenty-one direct paired opt-level3 comparisons against the dilation baseline,
all byte-identical (126 pairs), gave these candidate/baseline median ratios:

| Image | Preset 6 | Preset 8 |
|---|---:|---:|
| CID22 | 0.9598 | 0.8365 |
| CLIC2025 | 0.9890 | 0.9625 |
| terminal | 0.9815 | 0.9359 |

Records: `benchmarks/still_i265_2026-09-06-pd0-quant-ab.{tsv,raw.tsv,meta.json}`.
The default release profile has not changed. The broader identity sweeps and fresh C positions are recorded below.

## PD0 coefficient energy and distortion reuse

The next change routes PD0's packed frequency-domain distortion through the
existing `sse_i32` kernel. Its discarded-quadrant energy uses `sq_sum_i32`,
with one call for contiguous regions and one per row for the right quadrant.
Disassembly confirms the existing V3 energy kernel emits vector signed
multiplications and 64-bit accumulation; PD0 previously inlined scalar i64
multiplications. No new Archmage operation is required.

A new C parity test covers energy on all relevant rectangle sizes, padded
rows, unaligned element offsets and large padding sentinels under token
permutations. Omitting the final row deliberately failed; the correct code
was restored. The combined PD0 changes pass 2559/2559 workspace tests and
104/104 regression spot-checks. The default full identity sweep passed
1100/1100 with zero pinned cells and zero harness errors. Its summary and raw
file hash are in `benchmarks/still_i265_2026-09-06-pd0-identity.meta.json`.
The full real-corpus sweep also passed 450/450: six images each from CID22,
gb82 and gb82-sc, QP 5/20/32/48/63, presets 0/4/6/10/13. There were no
pinned cells or harness errors. With the local override removed, final checks
against the tracked cc24398c dependency and lockfile passed: nextest
2559/2559, zero skipped (34.274s test time), and regression spot-check
104/104. Both run-heavy invocations exited zero; logs are
`nextest-pinned-pd0.log` and `spotcheck-pinned-pd0.log` in the scratch directory.

Twenty-one pairs against the quantization-only candidate, all byte-identical,
gave the following incremental ratios:

| Image | Preset 6 | Preset 8 |
|---|---:|---:|
| CID22 | 0.9947 | 0.9884 |
| CLIC2025 | 0.9990 | 0.9862 |
| terminal | 0.9978 | 0.9855 |

Record: `benchmarks/still_i265_2026-09-06-pd0-energy-ab.{tsv,raw.tsv,meta.json}`.
Preset 8 gains are about 1–1.5%; preset 6 changes are small or noisy.

A fresh nine-pair C position, with all 81 pairs byte-identical, gives:

| Image | Preset 2 | Preset 6 | Preset 8 |
|---|---:|---:|---:|
| CID22 | 1.3855 | 1.5603 | 1.3987 |
| CLIC2025 | 1.7291 | 1.7440 | 1.6424 |
| terminal | 1.6007 | 1.5396 | 1.5795 |

Record: `benchmarks/still_i265_2026-09-06-pd0-position.{tsv,raw.tsv,meta.json}`.
Two of nine initial cells now meet the target, but seven still fail. These
512x512 QP40 results do not prove the goal across other sizes and qualities.


## Full-width SSE composition experiment

After PD0, a fresh CLIC 512x512 QP40 preset2 profile retained 152,810
cpu_core/cycles/u samples and reported 758 lost samples. It covers the entire
process (ten warmups plus the final encode), not an encode-only region.
Self shares: optimize_b 11.56%, tx_unit_inner 8.94%, directional predictors
4.50% and 3.77%, variance SSE 3.06%. The compact ranking and exact command
are in `benchmarks/still_i265_2026-09-06-clic-p2-pd0-profile.{tsv,meta.json}`.
An initial invocation with the wrong input prefix panicked and was discarded;
the reported profile uses the required raw: input and exited zero.

A standalone forbid-unsafe probe compares seven complete 16/32-byte SSE
compositions using the current PR97 snapshot. Its tests exhaust all 65,536
ordered constant-byte pairs plus lane-varying patterns. The 16-byte generic
widen-before-subtract and safely zero-padded variants produce byte-for-byte
identical 54-byte function bodies to explicit intrinsics, including reduction.
For 32 bytes the generic signed-difference version has the same instructions
as the intrinsic version with a different instruction ordering. The existing
absdiff-before-widen version takes extra operations. This is consumer-shape
evidence: no new API is necessary to express the wider arithmetic. It does
not prove a frame speedup, nor an ARM/WASM result. Scratch sources and binary:
`~/tmp/still-perf-2026-09-06/sse-probe/`.

The experimental encoder change uses that wider composition for AVX2,
sharing the existing row/packing/drain loop with scalar/WASM and retaining
the separate NEON implementation. All 22 selected variance tests passed
against the tracked cc24398c dependency (551 unrelated tests excluded by the
explicit filter). The production disassembly contains direct 16-byte memory
widenings and a single 256-bit madd for the full-width fold. Both comparison
binaries were built with the same tracked dependency, opt-level3 and baseline
target CPU. The initial nine-pair comparison completed with all 81 pairs byte-identical.
It gives no consistent gain and includes small regressions; record
`benchmarks/still_i265_2026-09-06-sse-wide-ab.{tsv,raw.tsv,meta.json}`.
Production disassembly still calls memcpy in the dynamic-width narrow-row
packing loop. The next experiment specializes widths 4 and 8 to remove
that dynamic copying. The final change combines wider arithmetic with specialized narrow packing.


The narrow-width candidate passes the same 22 selected variance tests. A
width4 result-plus-one mutation was rejected by both the exhaustive shape
comparison and the real C SSE comparison, then restored. The single
sse_all_dispatch_levels test did not detect that mutation because its shape
is not width4; it cannot alone validate this change. The initial wide-only
candidate and the narrow-specialized candidate are saved as
`perf_encode.sse-wide-pinned` and `perf_encode.sse-narrow-pinned`; the fresh
matching baseline is `perf_encode.pd0-energy-pinned`, all in the scratch
directory. The next paired run is `sse-narrow-ab`, nine pairs on all initial
cells. No whole-frame gain is claimed before reading its terminal result.


The completed narrow-width run kept all 81 pairs byte-identical. Relative to
PD0, median ratios (p2/p6/p8) were CID22 0.9859/0.9903/0.9953,
CLIC 0.9921/1.0007/0.9994, terminal 0.9910/0.9927/0.9819. CID22 p2/p6
and terminal p2/p6/p8 have interquartile intervals below 1.0; the remaining
cells are inconclusive. The same-binary control (21 pairs for each p6 cell,
all byte-identical) reads 1.0003/0.9969/0.9996, with all intervals including
1.0. This supports a small targeted improvement, not a general 1.5% claim.
Records: `benchmarks/still_i265_2026-09-06-sse-narrow-ab.*` and
`benchmarks/still_i265_2026-09-06-sse-control.*`. The arithmetic-only result
remains a negative finding; fixed-width packing is necessary for this result.

The final SSE tree passed 2559/2559 workspace nextest tests, zero skipped,
and 104/104 regression spot-checks against the tracked dependency; both
run-heavy invocations exited zero. Logs: `sse-nextest.log` and
`sse-spotcheck.log` in the scratch directory.


## Precomputed quantizer rows (not adopted)

The C builder fills all 256 rows at sequence initialization; Rust rebuilt
rows inside transform quantization, including repeated integer divisions.
A new differential shim calls the real svt_av1_build_quantizer with base
qindex31 and zero delta-q. The new test compares every field and replicated
AC lane for 256 qindices, bit depths8/10, sharpness -7/-1/0/1/7. It passed
before the implementation change. This closes a gap in the prior quantizer
tests, which supplied Rust-built tables to the C quantization kernels.

The candidate stores two compile-time default tables and adjusts only
zbin/round for nonzero sharpness. PD0 reuses the existing public quantizer
row builder. The C table test and PD0 quantization parity passed afterward.
A deliberately wrong default-row index failed the new test, then was restored
and the test passed again. Logs: quant-tables-baseline.log,
quant-cache-tests.log, quant-cache-mutation.log, quant-cache-restored.log.
The identical-settings opt3 comparison against the landed SSE candidate
completed with all 81 pairs byte-identical. CID22 p8 improved 1.27%, but
terminal p2 regressed 0.55%, and most other cells were inconclusive. The
production cache was not adopted: it does not demonstrate progress on the
remaining failing cells. Its source remains recoverable as jj change
zsyorrxr; the direct C table oracle is retained. Records:
`benchmarks/still_i265_2026-09-06-quant-cache-ab.*`.


A matching C CLIC p2 profile (same input/config, ten warmups plus one encode,
performance core2) retained18,293 cpu_core/cycles/u samples, zero lost.
C RDOQ self share is18.94% against the earlier Rust11.56%; after accounting
for Rust's ~1.73x whole-frame time these are similar absolute costs, so the
largest Rust symbol alone is not the right optimization priority. Directional
prediction is a stronger lead: Rust dr_z2_edged_core4.50% plus
 dr_predictor_edged3.77%, against C's zone2AVX2 2.74% and predictor/build
wrappers1.27%/1.16%. These are whole-process profiles and approximate
cross-run attribution, not a precise isolated kernel speed comparison.
The x86 zone2 source currently falls back to the scalar core; the flat-input
SIMD specialization exists only on NEON. The compact C ranking and exact
command are recorded beside the Rust profile.


## Frame screen-content derivation reuse

The immutable encode_input is detected at frame setup, then encode_tile_rows
repeated the same pure derive_sc call inside each tile closure, with the same
arm/preset/dimensions. The candidate passes the Copy ScDerivation into the
tile walk, preserving the resolved tune-IQ detector preset and all readers.
The inter-frame persistence question is separate; this only removes same-frame
recomputation. It does not change which frame's pixels are classified.

Nine paired opt3 rounds on the initial nine cells were all byte-identical.
CLIC p6 improved2.22% (interquartile ratio0.9756–0.9865), terminal p6
0.43%, and other cells were small/noisy or slightly regressed. Preset8's
detector is disabled, so it serves as a useful negative control. Record:
`benchmarks/still_i265_2026-09-06-sc-reuse-ab.*`. No broad speedup is claimed.

Replacing the shared tile result with ScDerivation::default deliberately
failed regression_spotcheck (exit1), including screen/qp0 and video-key
palette cells. The correct frame_sc copy was restored. Final nextest passed2560/2560, zero skipped; regression spot-check104/104;
full identity1100/1100 and real identity450/450, each with zero pinned cells
and zero harness errors. Source remained frozen throughout. The quantizer
cache is absent; the new C quantizer-table test remains included. The summary
and raw-file hashes are recorded in
`benchmarks/still_i265_2026-09-06-sc-reuse-identity.meta.json`.


## Thin-LTO experiment (not adopted)

The same source fda1ed60 was built at opt-level3, baseline target CPU,
16 codegen units, with CARGO_PROFILE_RELEASE_LTO=thin. The baseline also
uses opt3 and baseline CPU, without cross-crate LTO. Build command is in
`~/tmp/still-perf-2026-09-06/build-thin.sh`; it explicitly clears RUSTFLAGS
and CARGO_ENCODED_RUSTFLAGS. The build took19s, peak RSS0.88GiB; this is an
observed build duration, not a controlled clean-build compile-cost delta.
GNU size reports text3,644,619 bytes baseline and3,671,193 bytes thin-LTO.

All81 paired outputs remained byte-identical. The result is mixed: small
CID22 gains and terminal p8 gain, but CLIC p2 regresses about1.25% and p6
about0.57%. The release profile remains unchanged. Records:
`benchmarks/still_i265_2026-09-06-thin-ab.*`. This is a Rust-vs-Rust
experiment, not a comparison of LTO-enabled Rust against LTO-disabled C.

The next compiler experiment may use PGO with distinct training images.
The official rustc PGO workflow requires matching generation/use flags,
absolute profile paths, and an explicit target to keep instrumentation out
of build scripts: https://doc.rust-lang.org/rustc/profile-guided-optimization.html.
LLVM BOLT's official workflow is https://github.com/llvm/llvm-project/tree/main/bolt.
At this stage neither PGO nor BOLT had been measured; the later PGO sections
record the completed compiler experiment. BOLT remains unmeasured.

## Zone-2 directional prediction: split edge runs

The scalar zone-2 loop chooses above or left for every pixel. Each row
has a single transition between those edges. The candidate computes that
column once, evaluates the left prefix with C's scalar indexing, and
interpolates the contiguous above suffix with slice iterators. Const
upsampling flags keep the source step at one or two bytes. One Archmage
V3 boundary covers the block; no new dependency APIs or raw intrinsics
are needed. Other architectures retain their existing implementation.

On i265, the production opt3 function is 3,926 bytes. Disassembly confirms
AVX2 multiply/add/shift/pack operations on the above suffix. This does not
mean every short row vectorizes: the emitted vector loop consumes 32
outputs per iteration, with scalar tails. The 32-bit arithmetic also
warrants a separate experiment with bounded 16-bit intermediates.

The exploratory kernel probe covers five square sizes and five angles,
with 21 randomized pairs each. Its candidate receives a held token while
the baseline uses public dispatch, so it is not an abstraction-overhead
comparison. Ratios and raw-file/source hashes are in
`benchmarks/still_i265_2026-09-06-dr-zone2-probe.*`; its source is preserved
in `tools/perf_profile/dr_zone2_probe/`. Its real-C test passed 2,394 cases
including padded rows and trailing sentinels.

The existing production all-tier C test was strengthened to all seven
angle deltas and 19 shapes. It passed before the new implementation and
afterward. Changing the above interpolation rounding from +16 to +15
failed on 4x4 angle93 with above upsampling; restoring +16 passed again.

Nine paired whole-frame rounds against `fda1ed60` at opt3, baseline target
CPU, and CPU affinity2 produced identical output on all 81 pairs:

| Image | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID22 | 0.9898 | 0.9955 | 0.9993 |
| CLIC2025 | 0.9768 | 0.9939 | 0.9942 |
| terminal | 0.9889 | 0.9928 | 0.9990 |

These are candidate/previous-Rust ratios, not port/C positions. The clearest
frame improvement is CLIC p2 (2.32%); p8 spans overlap one. The benchmark
wrapper completed in 120s, with peak RSS 0.03GiB for its monitored process
and minimum available memory 26,313MiB; that RSS is not an encoder memory
measurement. Records: `benchmarks/still_i265_2026-09-06-dr-zone2-ab.*`.
Final workspace nextest passed 2560/2560 with zero skipped, and the
regression spot-check passed 104/104. The DSP formatting check passed.
Clippy completed with existing warnings elsewhere; the new dispatch
condition was collapsed to remove its one new warning. The archived probe
also passed its real-C test from the tracked relative-path manifest.

The same-session same-binary control ran 21 pairs on each of the three
p6 cells (63/63 identical), with ratios 0.9988/1.0026/1.0009; all
interquartile spans contain one. This control does not independently
price p2/p8 noise. Records: `benchmarks/still_i265_2026-09-06-dr-zone2-control.*`.

After the lint-only dispatch rewrite, an opt3 rebuild produced a byte-identical
`.text` section to the timed binary. The final binary and gate-log hashes are
in `benchmarks/still_i265_2026-09-06-dr-zone2-validation.meta.json`.

## Narrower zone-2 arithmetic (not adopted)

A follow-up probe replaced only above-edge i32 products/sums with u16.
The mathematical bound is 255*32+16 = 8176, so overflow is not needed.
Its real-C test passed all 2,394 padded-buffer cases. However, 21-pair
native microbenchmarks against the landed split loop regressed by 7.75–9.53%
on 16x16 at angles113/104 and 5.03–8.09% on 32x32. The candidate already
had the favorable held-token entry path. Some other shapes improved or
were noisy. No production change or frame-speed claim follows from it.
Records: `benchmarks/still_i265_2026-09-06-dr-u16-probe.*`; reproduce the
variant by changing only the above interpolation's operands and weight
to u16 in the preserved probe. Smaller integer types alone are not
evidence of cheaper generated code.

## PGO with separate training images

`tools/perf_profile/pgo_still.py` builds ordinary, instrumented, and PGO
drivers from the same frozen source. All use opt3, 16 codegen units, no
LTO, and explicit x86_64-unknown-linux-gnu with baseline target CPU. The
profile-use build adds the missing-function diagnostic. The workflow follows
[the rustc PGO documentation](https://doc.rust-lang.org/rustc/profile-guided-optimization.html).

Training uses rain, sunset, night, and windows95 from codec-corpus, at
64/256/512 crops (windows95's largest is 480, its source height), QP20/40/60,
and presets2/6/8. The 12 crops yield 108 cells. Their source and I420 hashes
have no overlap with the six-image, 15-crop evaluation manifest.
`prepare_still_inputs.py` reproduces both datasets; all 27 crops matched the
earlier preparation byte-for-byte, including all three original Rust-generated
512 YUV controls. The initial attempt correctly refused a 512 crop from the
640x480 windows95 source; no upscaling or padding was substituted.

All 108 instrumented training outputs matched the ordinary Rust driver.
The merged profile has 3,279 functions and 19,141,257,707 block counts;
optimize_b and tx_unit_inner have nonzero counts. The profile-use build
reports 105 missing-function warnings, including unused API functions and
small identity transforms; nine names have out-of-line symbols in the
instrumented executable. No hash-mismatch or changed-control-flow diagnostic
was reported. These warnings remain recorded, rather than suppressed.
The complete build/train/use job took 62s under run-heavy, peak monitored
RSS0.89GiB and minimum available memory25,543MiB; this is not a controlled
compile-cost comparison. Large profiles/logs stay outside git with hashes in
`benchmarks/still_i265_2026-09-06-pgo-training.meta.json`.

Nine pairs on each initial held-out cell were all byte-identical:

| Image | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID22 | 0.9070 | 0.9309 | 0.9435 |
| CLIC2025 | 0.9194 | 0.9552 | 0.9862 |
| terminal | 0.9105 | 0.9445 | 0.9425 |

These are PGO/ordinary Rust ratios, not PGO Rust/C comparisons. All nine
interquartile spans are below one. GNU size text falls from3,646,395 to
3,429,835 bytes. Records: `benchmarks/still_i265_2026-09-06-pgo-first-ab.*`.
The following section compares optimized Rust against C trained on the
same cells. No release profile changed.

Additional tools installed and exercised: cargo-pgo0.3.0, rustfilt0.2.1,
LLVM BOLT22.1.2, and measureme summarize12.0.3 at git e22edccd. Summarize
accepts subcommands and --help, but has no --version option. Rust's matching
llvm-tools-preview component provides llvm-profdata22.1.8-rust-1.98.1-stable,
which merged and read the profile used above. BOLT has not optimized a binary.

## Position with both encoders trained

GCC15.2 PGO was built from the same pinned C3115c0c source, Release-O3,
NATIVE=OFF, AVX512 enabled for runtime dispatch, no LTO, HDR off. Its
instrumented build and static driver use -fprofile-generate and atomic
counter updates; -fprofile-use reads the resulting directory from the
same build path. The driver retains its existing -O2 setting. All output
artifacts are isolated from the ordinary oracle. The workflow is preserved
in `tools/perf_profile/pgo_c_still.py` and follows
[GCC's instrumentation options](https://gcc.gnu.org/onlinedocs/gcc/Instrumentation-Options.html).

C trained on exactly the same 108 cells as Rust. Every instrumented C
output matched ordinary C, and all 108 C training outputs matched Rust.
GCC reported five missing-profile files, all in safestringlib; its dump
contains nonzero counters and runs=108. The source submodule remained
clean. The two-build/train job completed in85s, peak monitored RSS0.37GiB,
minimum available memory26,209MiB. Profile/log hashes are recorded in
`benchmarks/still_i265_2026-09-06-c-pgo-training.meta.json`.

The fresh nine-pair Rust-PGO/C-PGO position on the initial grid is:

| Image | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID22 | 1.2912 | 1.5005 | 1.3416 |
| CLIC2025 | 1.6088 | 1.6573 | 1.6752 |
| terminal | 1.4801 | 1.4924 | 1.5349 |

All81 pairs are byte-identical. Four of nine medians are below1.50;
terminal p6's interquartile span still crosses1.50. CLIC's three cells
remain clearly outside the goal. PGO helps Rust, but training C too
prevents claiming that the compiler experiment alone closed the gap.
Records: `benchmarks/still_i265_2026-09-06-pgo-both-position.*`.
The following section records the completed 135-cell evaluation.

## Broader real-image position

Five paired rounds across six images, 15 center crops (256/512/1024 as
source dimensions permit), QP20/40/60, and presets2/6/8 completed in430s.
Both encoders use PGO trained on the same separate source images, baseline
target CPU, and core2. All675 output pairs were byte-identical. Only45 of
135 median ratios are at most1.50;42 have their p75 at most1.50.

| Source | Cells | Medians at most1.50 | Ratio range |
|---|---:|---:|---:|
| CID22 | 18 | 11 | 1.187–1.886 |
| CLIC2025 | 27 | 8 | 1.352–1.968 |
| terminal | 27 | 10 | 1.285–1.923 |
| waves | 18 | 6 | 1.302–1.986 |
| nyc | 18 | 6 | 1.332–2.047 |
| wiki | 27 | 4 | 1.413–2.095 |

The largest remaining ratio is wiki1024/QP60/p8 (2.0947); nyc512/QP60/p2
is2.0470. The high-QP cases need attention, not merely more repeats of
the initial QP40 grid. The target remains unmet. Summary and hashes for
the larger raw/metadata files are in
`benchmarks/still_i265_2026-09-06-pgo-broad-position.*`. Raw profiles and
measurements remain in the dated scratch directory; no encoder memory
claim is derived from the benchmark runner's monitored RSS.

C's own PGO/ordinary comparison passed81/81 pairs and improved all nine
initial medians by1.91–4.07%. Its21-pair same-binary controls on the three
p6 cells all have interquartile spans containing one. These runs also
exercise the runner's new explicit `--port-kind c` option, so C-vs-C
measurements have the correct driver CLI and binary-kind metadata.
Records: `benchmarks/still_i265_2026-09-06-c-pgo-{ab,control}.*`.

## CLIC preset8 profile after PGO

New paired hardware profiles explicitly use cpu_core/cycles/u, period200000,
core2, and dwarf8192 call stacks. Rust retained64,767 samples and C34,478,
with zero lost samples in either. Both perform400 warmups plus one final
encode and produce identical output. They include setup, teardown, and
logging, so their self shares are whole-process localization evidence.
Compact symbol tables and hashes are in
`benchmarks/still_i265_2026-09-06-clic-p8-pgo-{rust,c}-profile.*`.

Rust's leading self rows are square DCT dispatch/body8.49%, 8x8 Hadamard
7.16%, DC prediction4.53%, coefficient writer4.27%, and optimize_b4.24%.
The V3 Hadamard8 wrapper still calls the scalar column/transposed-buffer
body; its NEON sibling already uses vertical vector butterflies. DC
prediction still writes each pixel through nested indexed loops. These
are concrete next kernel experiments. The film-grain result remains
discarded in source, but source presence alone does not establish runtime
cost, so no gain is assumed from removing that call.


## V3 Hadamard 8x8 vector butterflies

The x86 V3 wrapper now performs both passes with the existing generic i16x8
addition/subtraction API and fixed-array transposes. It preserves C's positional
coefficient permutation and wrapping i16 results. No Archmage API or dependency
pin changed. The NEON and scalar paths retain their previous implementations.

A direct real-C test was added before changing the production body. It covers
1,200 padded-buffer cases per available token permutation, including 8-bit and
10-bit residuals, full-range i16 patterns, and output sentinels. Changing the
V3 butterfly's c2-c6 to c2+c6 made it fail at stride8/pattern11; restoring the
body passed. All 2,561 workspace nextests passed with zero skipped, and the
regression spot-check passed 104/104. These are DSP correctness checks, not a
new full-envelope identity sweep.

The preserved `tools/perf_profile/hadamard_probe` contains the frozen pre-change
algorithm, candidate, C tests, and a same-function control mode. The archived
native comparison reduced kernel time by 53.35–54.74% on strides8/16/32, from
33.53–34.52ns to 15.54–15.64ns. Each A/B group completed30 rounds; controls
completed40–100 rounds and all difference intervals contained zero. The candidate
has a held token while the baseline retains runtime dispatch, so frame measurements
are needed. A staged 1/2/4-word interleave transpose gave no additional benefit;
the simpler direct-array transpose was retained. Its generated code still contains
many blends and shuffles: this is not evidence of an optimal transpose.

Zenbench0.1.9's process scan on this Linux host counted its own heartbeat thread
as a competing benchmark. The unmodified attempt was stopped without a result.
A local copy excludes `/proc/self/task` entries; every completed probe reported
zero gate waits and three groups with at least30 rounds. The patch and reproduction
instructions are preserved with the probe. The original production-linked probe
and the archived frozen-baseline probe have separate recorded rows. An attempted
`--help` invocation also started measurements because the embedded macro does not
implement help; it was stopped and supplies no performance evidence.

Nine frame pairs per initial cell, opt3 baseline CPU, compared the production
candidate against the saved zone-2 binary. All81 pairs were byte-identical:

| Image | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID22 | 0.9996 | 0.9999 | 0.9903 |
| CLIC2025 | 0.9787 | 0.9913 | 0.9673 |
| terminal | 0.9858 | 0.9857 | 0.9701 |

Ratios are candidate/previous Rust, without PGO. CID22 p2/p6 are null results;
the other seven interquartile spans are below one. Fresh21-pair controls on each
p6 cell are1.0006/1.0002/1.0004, all with interquartile spans containing one.
The control does not independently price p2/p8 noise. The A/B completed in118s
under run-heavy, peak monitored RSS0.03GiB, minimum available memory29,368MiB;
this is the wrapper's resource guard, not an encoder memory measurement.
Records: `benchmarks/still_i265_2026-09-06-hadamard-{probe,ab,control,validation}.*`.
The broader matched-PGO position above predates this change and must be remeasured
with fresh training profiles before claiming progress on that grid.


## DC prediction: fill contiguous output spans

The DC predictor's mean and rounding stay the same. Tightly packed blocks now
use one slice fill; other strides use one fill per row. Empty dimensions return
after the existing mean calculation, preserving its validation behavior.
Disassembly confirms one tail-call to memset on the tight path and one call per
row on the strided path. No SIMD dispatch or Archmage API was added.

Before the change, a direct C shim and test compared all19 block shapes, four
edge choices, four strides (including overlapping rows), and32 patterns:9,728
cases. Both input offsets and output sentinels are checked. A dc+1 mutation in
the tight path failed at4x4/stride4/no edges; the correct body then passed2,562
workspace nextests with zero skipped and104/104 regression spot-checks. The
reference submodule was not modified; the shim calls its sized 8-bit functions.

Nine frame pairs per initial cell compared against the saved Hadamard binary,
with opt3 and baseline CPU. All81 pairs were byte-identical:

| Image | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID22 | 0.9977 | 0.9887 | 0.9672 |
| CLIC2025 | 0.9879 | 0.9912 | 0.9709 |
| terminal | 1.0000 | 0.9914 | 0.9715 |

Ratios are candidate/previous Rust without PGO. CID22 and terminal p2 are null
results; the other seven interquartile spans are below one. Fresh21-pair p6
same-binary controls read0.9994/1.0003/1.0000, all spanning one. Those controls
do not independently measure p2/p8 noise. The A/B took117s under run-heavy,
peak monitored RSS0.03GiB and minimum available memory29,369MiB.
Records: `benchmarks/still_i265_2026-09-06-dc-fill-{ab,control,validation}.*`.
The earlier broad PGO position still predates both this and the Hadamard change.


## Matched PGO after Hadamard and DC fills

The resumed measurements run on Linux7.0.0-31-generic; the initial host
record above was7.0.0-30. Fresh Rust PGO training at `e711ee08` covers the same 108 cells as the unchanged
C PGO binary. All training outputs match C and the previous training hashes.
The Rust profile-use build retains 105 missing-function warnings, as before;
this experiment does not change the workspace release defaults.

The held-out grid repeats 135 cells five times, alternating paired arms on
core 2 with baseline CPU targeting. All 675 pairs are byte-identical.
**64/135 median ratios and 63/135 upper-quartile ratios meet 1.50**, compared
with 45 and 42 before the two kernel changes. The target remains unmet.
The worst median is NYC512/QP60/p2 at **1.9651** (Rust446.44ms, C227.51ms).
Wiki1024/QP60/p8 is **1.9085** (Rust12.64ms, C6.60ms), previously2.0947.
The sweep took418s under run-heavy, peak monitored RSS0.07GiB, minimum
available memory29,262MiB. Records:
`benchmarks/still_i265_2026-09-06-pgo-had-dc-{training,broad}.*`.

Paired Linux perf captures use these same binaries and inputs. NYC has20
warmups plus a final encode, wiki400 plus a final encode. Both arms produce
identical output and report zero lost samples: NYC254,458 Rust/128,722 C;
wiki135,143 Rust/74,872 C. These are whole-process self shares, including
initialization, teardown, and logging; they are not encode-only timings.
With perf_event_paranoid back at4, `profile_still.py --sudo-perf` uses scoped
`sudo -n perf record`, including its child driver, without changing sysctls.
The unprivileged attempt failed before recording any samples.

NYC's leading Rust self rows are the TXT search closure7.64%, zone-2 prediction
7.05%, square DCT4.75%, Hadamard8 3.82%, and tx_unit3.81%. C's leading rows
are fdct32 7.70%, fdct64 6.81%, zone-2 4.99%, optimize_b4.42%, and transform-type
search4.24%. Inlined work can be charged to the Rust closure; its name alone
does not identify the expensive operation.

Wiki's leading Rust rows are square DCT10.05%, fdct64 9.26%, Hadamard8 6.46%,
memset5.48%, evaluate_leaf4.99%, Hadamard32 4.91%, and idct64 4.70%.
C's leaders are fdct64 15.48%, fdct32 5.66%, Hadamard32 4.29%, internal memcpy
3.97%, and forward64 wrapper2.71%. These ranks suggest different optimization
priorities across presets. Compact tables and raw-artifact hashes are in
`benchmarks/still_i265_2026-09-06-{nyc-q60-p2,wiki-q60-p8}-*-profile.tsv`
and the corresponding `*-profile.meta.json`. Large perf recordings remain
outside Git in the recorded scratch directories.


## EOB zero-tail batching: measured, not adopted

Annotating the NYC PGO search closure assigns38.51% of its local samples to
the backward EOB scan loop (about2.94% of whole-process samples). A separate
coefficient-magnitude sum is also hot. The closure name had obscured these
inlined operations.

Two scalar probes batch eight scan-indexed coefficient loads. A per-lane
nonzero bitmask and a simpler OR reduction both preserve every tested EOB
and signed extreme. The OR form reduces long-tail kernel time by54–60%, but
short-tail cases regress24–46%. Same-function controls have several nonzero
difference intervals, including roughly9% and14% biases; their raw results
are retained, so small kernel effects are not clean evidence.

The production OR candidate keeps an immediate last-coefficient fast path,
then batches zero tails and uses an inline scalar walk for the first nonempty
chunk. Before changing the helper, a new real-C quantizer test passed every
possible EOB for all19 transform shapes and three scan classes, both FP and
B quantizers, against scalar and dispatched C. A subtract9-for8 mutation failed
at TX4x4/default scan/EOB7. The correct candidate passed2,563 workspace tests
with zero skipped and104/104 regression spot-checks.

Nine paired frame repeats on11 cells are all byte-identical. Candidate/previous
Rust ratios (opt3 baseline CPU, no PGO) are:

| Image/QP | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID512/QP40 | 1.0095 | 1.0018 | 1.0007 |
| CLIC512/QP40 | 0.9896 | 1.0100 | 0.9968 |
| terminal512/QP40 | 0.9983 | 1.0055 | 1.0059 |
| NYC512/QP60 | 0.9780 | — | — |
| wiki1024/QP60 | — | — | 0.9932 |

CID p2 and CLIC p6 regress about1%, despite the NYC gain. The production change
is preserved in local jj change `yswmwtlv` and is **not adopted**. No full
identity sweep or new PGO training is claimed for it. The additive sparse C
test and probe records are retained; the shipping encoder body is unchanged.
Records: `benchmarks/still_i265_2026-09-06-eob-{probe,frame-ab}.*`.
