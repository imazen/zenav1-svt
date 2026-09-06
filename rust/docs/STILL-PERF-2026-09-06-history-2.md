# Still-image performance: earlier experiments 2

[Current report](STILL-PERF-2026-09-06.md).

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
