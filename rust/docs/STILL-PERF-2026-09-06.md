# Still-image performance and SIMD API audit, 2026-09-06

The current task is to bring real-image still encoding within **1.50 times
C's time**, with matched tuning, byte-identical output, baseline target CPU
and runtime SIMD dispatch. It also includes installing and exercising the
profiling tools, evaluating Archmage PRs #96 and #97 for runtime and compile
cost, and identifying any additional operations the encoder actually needs.
This task is in progress; the first nine-cell position does not meet the target.

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
these arithmetic instructions. Whether a different full-width composition is cheaper remains unmeasured.
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
The exact driver remains in local scratch as `compile_cost.py`.

An initial invalid run used `cargo clean -p` without `--release`, removed
zero files, and measured cache hits. It was discarded. The corrected run uses
`cargo clean --release -p archmage -p magetypes -p compile_cost_probe` and
rejects any sample whose compiler-artifact messages report those crates fresh.

Current API assessment: the casts in #97 add no observed conversion cost to
the tested chains; #96's primitive and fused operations match their explicit
intrinsic references. Neither PR alone demonstrates an encoder speedup.
The current SSE consumer's split 128-bit arithmetic still needs comparison
against a full-width kernel. Keep specialized raw-intrinsic paths where a
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
