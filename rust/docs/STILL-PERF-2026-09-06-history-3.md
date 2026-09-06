# Still-image performance: earlier experiments 3

[Current report](STILL-PERF-2026-09-06.md).

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


## Hadamard transpose: explicit safe unpacks

The V3 butterfly arithmetic remains generic i16x8. Its two transposes now
use existing safe unpack16/32/64 intrinsics, with raw/from_m128i conversions.
No new Archmage API or dependency pin is needed. This addresses a composition
codegen problem: the fixed-array transpose emitted many blends. The production
kernel now has48 unpack instructions and zero blends, versus40 unpacks and68
blends before, and its disassembly shrinks from417 to243 lines including
padding and bounds-failure paths. There are no calls in the successful kernel
path. This is evidence for retaining the explicit transpose here, not for
expanding Archmage's API or claiming every generic composition is optimal.

The isolated frozen generic baseline and unpack candidate share a held V3
token and both match C on1,200 padded/full-range cases. Controls complete90
rounds per stride8/16/32 with difference intervals containing zero; A/B groups
complete30 each. All report zero gate waits. The candidate runs6.41–6.49ns,
versus23.17–23.22ns for this build's baseline. Earlier generic builds measured
15–16ns, so the isolated72% reduction is not a production gain estimate.

The production all-tier C test catches a deliberate unpacklo64(b1,b6)
substitution for unpacklo64(b1,b5) at stride8/pattern9. The correct body passes
all2,563 workspace tests with zero skipped and104/104 regression spot-checks.
The archived probe's C test also passes. No new full-envelope sweep is claimed
for this DSP-only change.

Nine frame pairs per11 cells compare against the saved DC-fill production
binary, opt3 baseline CPU without PGO. All99 pairs are byte-identical:

| Image/QP | Preset2 | Preset6 | Preset8 |
|---|---:|---:|---:|
| CID512/QP40 | 0.9968 | 0.9970 | 0.9867 |
| CLIC512/QP40 | 0.9801 | 0.9923 | 0.9672 |
| terminal512/QP40 | 0.9865 | 0.9907 | 0.9889 |
| NYC512/QP60 | 0.9667 | — | — |
| wiki1024/QP60 | — | — | 0.9398 |

Ratios are candidate/previous Rust. The largest gains are3.33% on NYC and6.02%
on wiki. CID p2/p6 are near the same-binary control noise, so no small gain is
asserted there. The matched-PGO grid above predates this transpose change.
Records: `benchmarks/still_i265_2026-09-06-hadamard-transpose-{probe,ab}.*`.

Fresh nine-pair same-binary controls cover all11 cells, all99 pairs identical.
Their median ratios range0.9940–1.0042. Three interquartile spans exclude one
(CID p6, CLIC p2, terminal p6); the control is not noiseless. NYC's control
span is0.9975–1.0150 versus the candidate's0.9653–0.9678; wiki's is0.9882–1.0098
versus0.9361–0.9418. These support the larger frame gains despite small biases.
The A/B and control resource logs are preserved with their metadata; control
runtime134s, peak monitored RSS0.03GiB, minimum available memory29,188MiB.
Control records: `benchmarks/still_i265_2026-09-06-hadamard-transpose-control.*`.


## Matched PGO after the unpack transpose

Fresh Rust training at `e1a555f4` again matches all108 C training-output hashes.
The build retains105 missing-function profile warnings. Baseline target CPU,
opt3, the same separate training images, and the unchanged C PGO binary keep
the comparison matched. The normal workspace release profile is unchanged.

All675 held-out pairs across135 cells are byte-identical. **73/135 median
ratios and69/135 upper-quartile ratios meet1.50**, versus64/63 before the
transpose change. The goal remains unmet in62 median cases. The worst is
NYC512/QP60/p2 at1.9139, down from1.9651. Wiki1024/QP60/p8 is1.8268, down from
1.9085. The corresponding preset6 ratio is1.8499. These are fresh
matched-PGO positions, separate from the non-PGO Rust/Rust A/B above.

The sweep takes413s under run-heavy, peak monitored RSS0.07GiB and minimum
available memory29,258MiB. Small tables and artifact hashes:
`benchmarks/still_i265_2026-09-06-pgo-had-unpack-{training,broad}.*`.

A scoped-root retry of VTune2026.4 software Hotspots collection passes the
previous ptrace restriction and finishes the NYC encode, but aborts with
`std::bad_alloc` (rc134). It also warns that p-core GP0 is in use. No usable
VTune profile is claimed, and no security sysctl was changed. The failed
capture directory and log remain in scratch; the training metadata records
the log hash. Linux perf remains the working sampling profiler.


## DCT64 precision specialization: not adopted

An isolated copy of the existing64x64 driver compares four alternatives:
constant-precision kernels called directly, a switch specializing10/13 with
a runtime fallback, a fixed-array output view, and a returned coefficient
array. Every variant matches the real C transform on160 blocks across
strides64/71, input offsets and output sentinels. The two constant variants
improve kernel time roughly6–8%; the fixed-buffer and returned-array variants
do not improve it. Controls complete100 rounds per stride with intervals
containing zero. All runs report zero gate waits.

The production-shaped switch candidate specializes only x86 and retains the
runtime path on ARM. Its20 C transform tests pass; deliberately using11 for
the10 arm fails the all-tier test at64x64/pattern2. The correct candidate
passes2,563 workspace tests with zero skipped and104 regression cases.
Cross-compiled ARM before/after kernel instruction text matches after numeric
label and panic-metadata normalization:4,006 instructions each. This is a
code-generation comparison, not an ARM runtime measurement.

The production binary's reported text size grows32,592 bytes (31.8KiB).
Nine frame pairs per11 cells are all byte-identical, but the speedup does not
carry through to encoding: NYC/QP60/p2 regresses0.89%, and wiki/QP60/p8 regresses
1.80%. CLIC p2/p6 also regress0.64%/0.41%; CID p6 improves0.78%. The candidate
is not adopted and is preserved in local jj change `lywlwvzz`. No PGO training
or broad-grid result is claimed for it; the shipping encoder stays at the
unpack-transpose implementation. The code-size observation alone does not
establish the cause of the frame regressions.

The frozen switch probe is in `tools/perf_profile/dct64_probe`; separate
patches reproduce the other three variants. Raw source, build and timing
hashes are in `benchmarks/still_i265_2026-09-06-dct64-{probe,frame-ab}.*`.


## Coefficient absolute-sum reductions

The narrow C-compatible SATD and the transform screen's existing i64 sum now
have x86 V3 dispatch around ordinary safe iterator reductions. LLVM generates
AVX2 loops without new intrinsics or Archmage API additions. Narrow inputs
below32 coefficients retain the scalar core: dispatch at16 regresses the
isolated probe by75%. The wide reduction preserves unsigned absolute values,
including i32::MIN, and the transform-screen decision arithmetic is unchanged.
Other architectures retain the existing inline transform-screen loop.

The direct test covers480 real-C cases per token permutation plus20 lengths
with closed-form sums beyond i32, including vector boundaries and offset input.
It passed before vectorization; deliberate narrow and wide V3 mutations each
fail it. The restored implementation passes all2,564 workspace tests with zero
skipped and104 regression spot-checks. The full synthetic/dimension gate
passes1,100/1,100 with zero pinned cases and zero harness errors (257s).
The real-image gate passes450/450, also with no pinned cases or harness
errors. Logs and table hashes are preserved in
`benchmarks/still_i265_2026-09-06-satd-identity.meta.json`.

The isolated dispatched wide sum improves31–69%; dispatched narrow sums at32
or more coefficients improve22–59%. The out-of-line probe baseline differs
from the encoder's formerly inline wide sum, so these are not frame gains.
Two16-coefficient control groups have nonzero intervals (about2% bias); all
runs report zero resource gate waits. The production text section grows3,320
bytes. The archived probe is `tools/perf_profile/satd_probe` and its records
are `benchmarks/still_i265_2026-09-06-satd-probe.*`.

Nine non-PGO frame pairs per11 cells all produce identical output. Median
candidate/previous-Rust ratios range0.9799–0.9978, including NYC512/QP60/p2
at0.9900 and wiki1024/QP60/p8 at0.9827. Fresh same-binary controls also produce99 identical pairs. Their medians
range0.9964–1.0019; CID p6 and terminal p2 interquartile spans exclude one.
Candidate gains of roughly1–2% support retention, while the smallest gains
remain uncertain. Frame records are `benchmarks/still_i265_2026-09-06-satd-frame-{ab,control}.*`.
Both full identity gates pass. The matched-PGO goal
position remains73/135 medians at or below1.50 until a fresh sweep is run.


### Next measured candidate: discarded film-grain estimate

The earlier wiki PGO profile assigns2.25% self samples to
`film_grain::estimate_film_grain`. The pipeline unconditionally calls this
homegrown heuristic after restoration and drops its plain-data return value.
Its entire body only reads source/reconstruction and builds that return value.
The actual fork photon-noise table comes separately from `noise_gen` before
encoding and is passed as `film_grain.as_ref()` to the header writer. C's
`svt_aom_picture_pre_processing_operations` instead guards its real denoiser
behind the film-grain configuration (pic_analysis_process.c:514–525).
Removing the discarded heuristic is a candidate for a separate measured change;
no production edit or speedup is claimed here. The module's historical claim
that all grain signaling is absent is stale for the fork photon-noise path.


### Follow-up Hadamard composition candidate

The16x16 and32x32 Hadamard wrappers remain ordinary baseline-CPU functions,
each recursively calling dispatched sub-transforms and then combining stored
i32 coefficients through i16 truncation/wrapping. The earlier wiki profile
assigns4.91% self samples to the32x32 wrapper. A held-token V3 implementation
could avoid repeated dispatch and expose wider combine loops. Any experiment
must preserve the explicit AVX2 high-bit-depth wrapping behavior documented
in `hadamard.rs` and test against the AVX2 C oracle, not just scalar C over
8-bit inputs. This is an investigation lead, with no candidate measurement yet.


## Matched PGO after coefficient reductions

Fresh baseline-CPU training of encoder source `a090303e` matches all108 C
training-output hashes and retains105 missing-function profile warnings.
The unchanged C PGO binary and separate held-out inputs keep the protocol
matched; normal release defaults remain unchanged.

All675 pairs across135 cases are byte-identical. **76/135 median ratios and
76/135 upper-quartile ratios meet1.50**, compared with73/69 before this change.
The goal remains unmet in59 median cases. NYC512/QP60/p2 remains worst at
1.8831 (previously1.9139), while wiki1024/QP60/p8 is1.7824 (previously1.8268).
Wiki preset6 remains1.8371. These counts describe this fresh run; individual
threshold crossings can be sensitive to timing noise.

Training takes60s and the held-out sweep412s under the shared wrapper. The
sweep's peak monitored RSS is0.07GiB, minimum available memory29,215MiB.
Tables and preserved artifact hashes are in
`benchmarks/still_i265_2026-09-06-pgo-satd-{training,broad}.*`.
